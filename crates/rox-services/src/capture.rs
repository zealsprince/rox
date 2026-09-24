//! Saving songs off the air, byte for byte: a station's own frames from
//! one in-band title change to the next, with no decoder or encoder.
//!
//! A capture counts only when it began at a boundary, ended at the next,
//! ran long enough to be a song, and had no reconnect in the middle. The
//! first capture of a connection is always thrown away: the song under the
//! first title was joined halfway. Stations flip titles a few seconds off
//! the audio, so captures carry slivers of the neighbouring songs; trimming
//! would need a silence detector capture doesn't have.
//!
//! Names come from the renamer's pattern language ([`rox_core::pattern`]).
//! A cover is written beside the file only if it clears the art matcher's
//! bar. Nothing here touches the decode thread: [`rox_playback::icy`] only
//! copies bytes into a channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use gpui::{Context, Entity, Subscription};

use rox_core::pattern::{Name, Pattern, PatternField};
use rox_core::settings::{self, Settings, safe_file_stem};
use rox_library::writer::{self, Change, Field};
use rox_playback::icy::{CaptureEvent, CaptureSink, IcyTitle};

use crate::catalog::Library;
use crate::player::Player;

/// The scrobbler's own floor (`MIN_TRACK_SECS`): under thirty seconds is an
/// ident, a trailer, or a bad boundary.
const MIN_SECS: f64 = 30.0;

/// Low on purpose, so the length guess errs towards keeping a song.
const ASSUMED_KBPS: u32 = 128;

/// Whatever the settings say: around half an hour at 320 kbps.
const CAP_BYTES: usize = 64 * 1024 * 1024;

/// The live buffer length: a song that never ends inside it (a podcast, a
/// mix) isn't a song to save. `apply` keeps it current.
static CEILING_SECS: AtomicU32 = AtomicU32::new(settings::DEFAULT_LIVE_BUFFER_SECS);

const MAX_COPIES: u32 = 999;

pub struct Take {
    pub title: IcyTitle,
    pub bytes: Vec<u8>,
}

struct InFlight {
    title: IcyTitle,
    bytes: Vec<u8>,
}

/// The start-to-finish rule with no app and no disk: events in, songs out.
pub struct Tape {
    current: Option<InFlight>,
    /// The next boundary is a tune-in announcement, not a song start. True
    /// at the start and again after every drop.
    joined: bool,
    min_bytes: usize,
    /// Kept rather than a byte ceiling: the ceiling follows the live buffer
    /// setting, which can move while a station plays.
    kbps: u32,
}

impl Default for Tape {
    fn default() -> Self {
        Tape {
            current: None,
            joined: true,
            min_bytes: min_bytes_at(ASSUMED_KBPS),
            kbps: ASSUMED_KBPS,
        }
    }
}

impl Tape {
    /// With no decoder behind a capture, byte count at the advertised rate is
    /// the only duration it can know.
    fn follow_bitrate(&mut self, kbps: u32) {
        self.min_bytes = min_bytes_at(kbps);
        self.kbps = kbps;
    }

    fn feed(&mut self, event: CaptureEvent) -> Option<Take> {
        match event {
            CaptureEvent::Boundary(title) => {
                // The first title on a connection opens nothing.
                if self.joined {
                    self.joined = false;
                    self.current = None;
                    return None;
                }

                let done = self
                    .current
                    .take()
                    .filter(|song| song.bytes.len() >= self.min_bytes);

                self.current = Some(InFlight {
                    title,
                    bytes: Vec::new(),
                });

                done.map(|song| Take {
                    title: song.title,
                    bytes: song.bytes,
                })
            }

            CaptureEvent::Bytes(mut bytes) => {
                // No song open: before the first boundary, or abandoned.
                let current = self.current.as_mut()?;

                // Past the ceiling this is a broadcast that never announced
                // its end. Log it, or a short buffer looks like capture doing
                // nothing.
                if current.bytes.len() + bytes.len() > max_bytes_at(self.kbps) {
                    log::info!(
                        "capture: dropping {} - {}: longer than the live buffer allows",
                        current.title.artist,
                        current.title.title
                    );
                    self.current = None;
                    return None;
                }

                current.bytes.append(&mut bytes);
                None
            }

            CaptureEvent::End => {
                // The connection dropped: the song in flight lost its middle.
                self.current = None;
                self.joined = true;
                None
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct Station {
    /// Both the station row's path and the comment the capture carries.
    url: String,
    name: String,
    genre: String,
    source: String,
    ext: &'static str,
}

/// The source id's kind with its first letter up, so `subsonic:<server>`
/// files as "Subsonic" and a future source names its own folder.
pub fn source_label(source: &str) -> String {
    let kind = source.split(':').next().unwrap_or(source).trim();
    let mut chars = kind.chars();

    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Every placeholder in the app-wide vocabulary parses; what the air
/// doesn't give renders as nothing.
#[derive(Clone, PartialEq)]
pub enum CaptureField {
    Artist,
    Title,
    /// Also `%album%`: a station is the only release a song off the air has.
    Station,
    Source,
    Genre,
    /// The container, "MP3". The header bitrate is station-wide, not this
    /// song's, so it isn't used.
    Format,
    Year,
    Date,
    /// Renders nothing rather than refusing a pattern the renamer accepts.
    Unfilled,
}

impl PatternField for CaptureField {
    fn from_name(name: Name) -> Option<Self> {
        Some(match name {
            Name::Artist => CaptureField::Artist,
            Name::Title => CaptureField::Title,
            Name::Album | Name::Station => CaptureField::Station,
            Name::Source => CaptureField::Source,
            Name::Genre => CaptureField::Genre,
            Name::Format => CaptureField::Format,
            Name::Year => CaptureField::Year,
            // The renamer reads %date% as the release year; here it's the
            // day the song was heard.
            Name::Date => CaptureField::Date,
            Name::AlbumArtist | Name::Track | Name::Disc | Name::Comment => CaptureField::Unfilled,
            Name::Skip => return None,
        })
    }

    fn fallback(&self) -> &'static str {
        match self {
            // Plenty of stations send one unsplittable title, and "Song"
            // beats "Unknown Artist - Song".
            CaptureField::Artist | CaptureField::Unfilled => "",

            CaptureField::Title => "Capture",

            CaptureField::Station => "Unknown Station",
            CaptureField::Source => "Unknown Source",
            CaptureField::Genre => "Unknown Genre",
            CaptureField::Format => "Unknown Format",

            // Unreachable from a real capture; kept total rather than panicking.
            CaptureField::Year => "Unknown Year",
            CaptureField::Date => "Unknown Date",
        }
    }
}

struct Stamp {
    day: String,
    year: String,
}

/// Local rather than UTC: the day a listener files a song under is the day
/// they heard it.
fn now() -> Stamp {
    let now = chrono::Local::now();

    Stamp {
        day: now.format("%Y-%m-%d").to_string(),
        year: now.format("%Y").to_string(),
    }
}

pub struct Capture {
    events: Receiver<CaptureEvent>,
    tape: Tape,
    /// Read as it plays, so a song is tagged with the station that played it
    /// rather than whatever is on when the write lands.
    station: Option<Station>,
    library: Entity<Library>,
    _player_changed: Subscription,
}

impl Capture {
    pub fn new(player: &Entity<Player>, library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // Unbounded on purpose: the pump drains it every 16 ms, and a bounded
        // channel would drop a batch and leave a hole in a capture.
        let (tx, events) = std::sync::mpsc::channel();
        rox_playback::icy::tee_to(sink(tx));
        apply();

        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });

        Capture {
            events,
            tape: Tape::default(),
            station: None,
            library: library.clone(),
            _player_changed,
        }
    }

    fn tick(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        // Before the drain: a song finishing now belongs to the station that
        // was on when its bytes arrived.
        self.follow(player, cx);

        let mut done = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            done.extend(self.tape.feed(event));
        }

        let Some(station) = self.station.clone() else {
            return;
        };

        for take in done {
            self.save(take, station.clone(), cx);
        }
    }

    /// A local file leaves the last station in place: the tape only fills
    /// from a stream.
    fn follow(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        let player = player.read(cx);

        let Some(now) = player.now_playing() else {
            return;
        };
        let Some(info) = player.station_info() else {
            return;
        };

        let station = Station {
            url: now.key.path.to_string_lossy().to_string(),
            name: info.name.clone(),
            genre: info.genre.clone(),
            source: source_label(&now.key.source),
            ext: rox_playback::http::extension_for(&info.content_type).unwrap_or("mp3"),
        };

        if self.station.as_ref() == Some(&station) {
            return;
        }

        self.tape.follow_bitrate(info.bitrate_kbps);
        self.station = Some(station);
    }

    fn save(&mut self, take: Take, station: Station, cx: &mut Context<Self>) {
        let capture = Settings::load().capture;
        let pattern = capture.parsed_pattern::<CaptureField>();
        let folder = capture.folder;
        let album = capture.album;
        let stamp = now();
        let library = self.library.clone();

        cx.spawn(async move |_, cx| {
            let written: Result<PathBuf, String> = cx
                .background_executor()
                .spawn(async move {
                    let path = write(&folder, &take, &station, &pattern, &album, &stamp)?;

                    // Before the reindex, or a thumbnail asked for on the
                    // repaint caches a definitive no-art answer.
                    save_cover(&path, &take.title);

                    Ok(path)
                })
                .await;

            match written {
                // Explicitly: the capture folder needn't be a library root.
                Ok(path) => {
                    log::info!("capture: saved {}", path.display());
                    library
                        .update(cx, |library, cx| library.reindex_written(vec![path], cx))
                        .ok();
                }

                Err(e) => log::warn!("capture: {e}"),
            }
        })
        .detach();
    }
}

/// Startup calls this once; the settings row calls it to make a change live.
pub fn apply() {
    let settings = Settings::load();
    rox_playback::icy::set_capturing(settings.capture.enabled);
    follow_live_buffer(settings.live_buffer_secs);
}

/// Called from the player's setter, so a slider change reaches a station
/// already playing.
pub fn follow_live_buffer(secs: u32) {
    CEILING_SECS.store(settings::clamp_live_buffer_secs(secs), Ordering::Relaxed);
}

/// Runs inside the decode thread's read, so a channel send and nothing else.
fn sink(tx: Sender<CaptureEvent>) -> CaptureSink {
    // A disconnected send means the app is going down.
    Arc::new(move |event| {
        let _ = tx.send(event);
    })
}

fn min_bytes_at(kbps: u32) -> usize {
    bytes_at(kbps, MIN_SECS)
}

fn max_bytes_at(kbps: u32) -> usize {
    bytes_at(kbps, f64::from(CEILING_SECS.load(Ordering::Relaxed))).min(CAP_BYTES)
}

fn bytes_at(kbps: u32, secs: f64) -> usize {
    let kbps = if kbps == 0 { ASSUMED_KBPS } else { kbps };

    (f64::from(kbps) * 1000.0 / 8.0 * secs) as usize
}

fn stem_for(title: &IcyTitle) -> String {
    let artist = title.artist.trim();
    let song = title.title.trim();

    let name = if artist.is_empty() {
        song.to_string()
    } else {
        format!("{artist} - {song}")
    };

    safe_file_stem(&name, "Capture")
}

fn free_path(
    folder: &Path,
    stem: &str,
    ext: &str,
    taken: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let first = folder.join(format!("{stem}.{ext}"));
    if !taken(&first) {
        return Some(first);
    }

    (2..=MAX_COPIES)
        .map(|n| folder.join(format!("{stem} ({n}).{ext}")))
        .find(|path| !taken(path))
}

/// The comment carries the station and its URL, the only provenance a song
/// off the air has. No album unless the setting asks: station names in the
/// album field pollute every album view.
fn tags_for(take: &Take, station: &Station, album: &str) -> Vec<Change> {
    let mut changes = vec![Change {
        field: Field::Title,
        value: Some(take.title.title.trim().to_string()),
    }];

    let mut set = |field: Field, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            changes.push(Change {
                field,
                value: Some(value.to_string()),
            });
        }
    };

    set(Field::Artist, &take.title.artist);
    set(Field::Album, album);
    set(Field::Genre, &station.genre);
    set(Field::Comment, &provenance(station));

    changes
}

fn provenance(station: &Station) -> String {
    match (station.name.trim(), station.url.trim()) {
        ("", url) => url.to_string(),
        (name, "") => name.to_string(),
        (name, url) => format!("{name} ({url})"),
    }
}

/// Text that isn't a pattern (a stray `%`) is written as typed: "Singles"
/// must never fail to be "Singles".
fn album_for(setting: &str, fields: &[(CaptureField, String)]) -> String {
    let setting = setting.trim();
    if setting.is_empty() {
        return String::new();
    }

    rox_core::pattern::parse::<CaptureField>(setting)
        .ok()
        .and_then(|pattern| pattern.render(fields).ok())
        .map(|rendered| rendered.to_string_lossy().trim().to_string())
        .unwrap_or_else(|| setting.to_string())
}

fn fields_for(title: &IcyTitle, station: &Station, stamp: &Stamp) -> Vec<(CaptureField, String)> {
    vec![
        (CaptureField::Artist, title.artist.trim().to_string()),
        (CaptureField::Title, title.title.trim().to_string()),
        (CaptureField::Station, station.name.clone()),
        (CaptureField::Source, station.source.clone()),
        (CaptureField::Genre, station.genre.clone()),
        (CaptureField::Format, station.ext.to_uppercase()),
        (CaptureField::Year, stamp.year.clone()),
        (CaptureField::Date, stamp.day.clone()),
    ]
}

/// Falls back to the flat [`stem_for`] name when the pattern renders
/// nothing.
fn relative_for(
    pattern: &Pattern<CaptureField>,
    fields: &[(CaptureField, String)],
    title: &IcyTitle,
) -> PathBuf {
    match pattern.render(fields) {
        Ok(rendered) => fold_segments(&rendered),

        Err(e) => {
            log::warn!("capture: the name pattern rendered nothing ({e})");
            PathBuf::from(stem_for(title))
        }
    }
}

/// Values were sanitized on the way in; the pattern's own literals weren't.
fn fold_segments(path: &Path) -> PathBuf {
    path.iter()
        .map(|segment| safe_file_stem(&segment.to_string_lossy(), "Capture"))
        .collect()
}

pub struct Sample {
    pub title: IcyTitle,
    pub station: String,
    pub source: String,
    pub genre: String,
    pub ext: &'static str,
}

impl Default for Sample {
    fn default() -> Self {
        Sample {
            title: IcyTitle {
                artist: "Aphex Twin".into(),
                title: "Xtal".into(),
            },
            station: "Noise FM".into(),
            source: source_label(rox_library::stations::SOURCE),
            genre: "Electronic".into(),
            ext: "mp3",
        }
    }
}

impl Sample {
    pub fn playing(player: &Player) -> Option<Sample> {
        let title = player.live_title()?;
        let info = player.station_info()?;
        let source = source_label(&player.now_playing()?.key.source);

        Some(Sample {
            title,
            station: info.name,
            source,
            genre: info.genre,
            ext: rox_playback::http::extension_for(&info.content_type).unwrap_or("mp3"),
        })
    }
}

/// Runs the same render a saved song takes, so the preview can't drift.
pub fn preview(pattern: &str, sample: &Sample) -> Result<String, String> {
    let parsed = rox_core::pattern::parse::<CaptureField>(pattern)?;
    let station = Station {
        url: String::new(),
        name: sample.station.clone(),
        genre: sample.genre.clone(),
        source: sample.source.clone(),
        // The container is appended below; a pattern never names it.
        ext: "",
    };

    let fields = fields_for(&sample.title, &station, &now());
    let rendered = fold_segments(&parsed.render(&fields)?);

    Ok(format!("{}.{}", rendered.display(), sample.ext))
}

/// Tags go through the writer's atomic layer (ADR 4). Blocking.
fn write(
    folder: &Path,
    take: &Take,
    station: &Station,
    pattern: &Pattern<CaptureField>,
    album: &str,
    stamp: &Stamp,
) -> Result<PathBuf, String> {
    let fields = fields_for(&take.title, station, stamp);
    let relative = relative_for(pattern, &fields, &take.title);
    let album = album_for(album, &fields);

    // Only the file name counts up through a collision; folders are shared.
    let dir = match relative.parent() {
        Some(parent) => folder.join(parent),
        None => folder.to_path_buf(),
    };
    let stem = relative.file_name().unwrap_or_default().to_string_lossy();

    std::fs::create_dir_all(&dir).map_err(|e| format!("making {}: {e}", dir.display()))?;

    let path = free_path(&dir, &stem, station.ext, &|path| path.exists())
        .ok_or_else(|| format!("no free name left for {stem}"))?;

    // Through a sibling temp file and a rename, so a watcher never indexes a
    // half-written capture as a truncated track.
    let part = path.with_extension(format!("{}.part", station.ext));
    std::fs::write(&part, &take.bytes).map_err(|e| format!("writing {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("naming {}: {e}", path.display()))?;

    // A mid-stream Ogg capture has no header pages for lofty to open, but it
    // still plays. Keep the file.
    if let Err(e) = writer::commit(&path, &tags_for(take, station, &album)) {
        log::warn!("capture: {} saved untagged: {e}", path.display());
    }

    Ok(path)
}

/// Named after the file, since a shared cover.jpg would be wrong for every
/// song but one; that's also what [`rox_library::art`] ranks first. The
/// backdrop already searched these names, so this hits the provider session
/// cache. Silent on failure: the cover never fails a capture.
fn save_cover(path: &Path, title: &IcyTitle) {
    let Some(query) = crate::radio_art::query(title) else {
        return;
    };
    let Some(bytes) = crate::radio_art::lookup(&query) else {
        return;
    };
    let Some(ext) = rox_library::art::image_extension(&bytes) else {
        return;
    };

    // Temp file and rename, so a reader never caches a half-written picture.
    let cover = path.with_extension(ext);
    let part = cover.with_extension(format!("{ext}.part"));
    let written = std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &cover));

    match written {
        Ok(()) => log::debug!("capture: cover at {}", cover.display()),
        Err(e) => log::debug!("capture: no cover for {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rox_core::settings::CaptureSettings;

    /// The ceiling is one global, so the tests that move it take turns.
    static CEILING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn title(artist: &str, song: &str) -> IcyTitle {
        IcyTitle {
            artist: artist.into(),
            title: song.into(),
        }
    }

    fn tape() -> Tape {
        Tape {
            min_bytes: 10,
            ..Tape::default()
        }
    }

    #[test]
    fn the_song_playing_when_we_tuned_in_is_not_saved() {
        let mut tape = tape();

        assert!(tape.feed(CaptureEvent::Boundary(title("A", "1"))).is_none());
        assert!(tape.feed(CaptureEvent::Bytes(vec![0; 400])).is_none());
        assert!(tape.feed(CaptureEvent::Boundary(title("B", "2"))).is_none());
    }

    #[test]
    fn a_song_heard_end_to_end_comes_back_whole() {
        let mut tape = tape();

        tape.feed(CaptureEvent::Boundary(title("A", "1")));
        tape.feed(CaptureEvent::Boundary(title("B", "2")));
        tape.feed(CaptureEvent::Bytes(vec![7; 8]));
        tape.feed(CaptureEvent::Bytes(vec![7; 8]));

        let take = tape
            .feed(CaptureEvent::Boundary(title("C", "3")))
            .expect("the second song ran its whole length");

        assert_eq!(take.title, title("B", "2"));
        assert_eq!(take.bytes, vec![7; 16]);
    }

    #[test]
    fn a_song_too_short_to_be_one_is_dropped() {
        let mut tape = tape();

        tape.feed(CaptureEvent::Boundary(title("A", "1")));
        tape.feed(CaptureEvent::Boundary(title("B", "2")));
        tape.feed(CaptureEvent::Bytes(vec![7; 4]));

        assert!(tape.feed(CaptureEvent::Boundary(title("C", "3"))).is_none());
    }

    #[test]
    fn a_reconnect_throws_the_song_in_flight_away() {
        let mut tape = tape();

        tape.feed(CaptureEvent::Boundary(title("A", "1")));
        tape.feed(CaptureEvent::Boundary(title("B", "2")));
        tape.feed(CaptureEvent::Bytes(vec![7; 400]));
        tape.feed(CaptureEvent::End);

        assert!(tape.feed(CaptureEvent::Boundary(title("C", "3"))).is_none());

        tape.feed(CaptureEvent::Bytes(vec![7; 400]));
        assert!(tape.feed(CaptureEvent::Boundary(title("D", "4"))).is_none());
    }

    #[test]
    fn a_capture_past_the_ceiling_is_given_up() {
        let _guard = CEILING_LOCK.lock().unwrap();
        let mut tape = tape();

        tape.feed(CaptureEvent::Boundary(title("A", "1")));
        tape.feed(CaptureEvent::Boundary(title("B", "2")));
        let ceiling = max_bytes_at(tape.kbps);
        tape.feed(CaptureEvent::Bytes(vec![0; ceiling]));
        tape.feed(CaptureEvent::Bytes(vec![0; 1]));

        assert!(tape.feed(CaptureEvent::Boundary(title("C", "3"))).is_none());
    }

    /// A song already taping is judged against the buffer as it is now, not
    /// as it was at connect.
    #[test]
    fn a_longer_buffer_reaches_a_song_in_flight() {
        let _guard = CEILING_LOCK.lock().unwrap();
        CEILING_SECS.store(60, Ordering::Relaxed);
        let mut tape = Tape::default();
        tape.follow_bitrate(128);
        tape.feed(CaptureEvent::Boundary(title("A", "First")));
        tape.feed(CaptureEvent::Boundary(title("B", "Second")));

        let chunk = vec![0u8; bytes_at(128, 30.0)];
        tape.feed(CaptureEvent::Bytes(chunk.clone()));
        tape.feed(CaptureEvent::Bytes(chunk.clone()));
        CEILING_SECS.store(1800, Ordering::Relaxed);
        tape.feed(CaptureEvent::Bytes(chunk.clone()));
        let took = tape.feed(CaptureEvent::Boundary(title("C", "Third")));
        assert!(took.is_some(), "the raised buffer kept the song");
        CEILING_SECS.store(settings::DEFAULT_LIVE_BUFFER_SECS, Ordering::Relaxed);
    }

    #[test]
    fn the_ceiling_is_the_live_buffer_at_the_stations_bitrate() {
        let _guard = CEILING_LOCK.lock().unwrap();
        CEILING_SECS.store(600, Ordering::Relaxed);
        assert_eq!(max_bytes_at(128), 9_600_000);
        assert_eq!(max_bytes_at(0), max_bytes_at(ASSUMED_KBPS));

        CEILING_SECS.store(3600, Ordering::Relaxed);
        assert_eq!(max_bytes_at(320), CAP_BYTES, "the hard cap holds");
        CEILING_SECS.store(settings::DEFAULT_LIVE_BUFFER_SECS, Ordering::Relaxed);
    }

    #[test]
    fn the_length_rule_follows_the_stations_bitrate() {
        // Thirty seconds at 128 kbps is 480 kB; at 320 it is 1.2 MB.
        assert_eq!(min_bytes_at(128), 480_000);
        assert_eq!(min_bytes_at(320), 1_200_000);
        assert_eq!(min_bytes_at(0), min_bytes_at(ASSUMED_KBPS));
    }

    #[test]
    fn a_name_that_cannot_be_a_filename_is_folded() {
        assert_eq!(
            stem_for(&title("AC/DC", "Back in Black")),
            "AC DC - Back in Black"
        );
        assert_eq!(stem_for(&title("", "NTS Radio 1")), "NTS Radio 1");
        assert_eq!(stem_for(&title("", "  ...  ")), "Capture");
    }

    fn station() -> Station {
        Station {
            url: "https://example.org/stream".into(),
            name: "Noise FM".into(),
            genre: "Electronic".into(),
            source: "Radio".into(),
            ext: "mp3",
        }
    }

    #[test]
    fn the_source_names_the_provider() {
        assert_eq!(source_label("radio"), "Radio");
        assert_eq!(source_label("subsonic:9620683f4e1f3ac2"), "Subsonic");
        assert_eq!(source_label("mixcloud"), "Mixcloud");
        assert_eq!(
            placed(&pattern("%source%/%station%/%title%"), "Aphex Twin", "Xtal"),
            "Radio/Noise FM/Xtal"
        );
    }

    fn placed(pattern: &Pattern<CaptureField>, artist: &str, song: &str) -> String {
        let title = title(artist, song);
        let stamp = Stamp {
            day: "2026-09-18".into(),
            year: "2026".into(),
        };

        relative_for(pattern, &fields_for(&title, &station(), &stamp), &title)
            .to_string_lossy()
            .into_owned()
    }

    fn pattern(text: &str) -> Pattern<CaptureField> {
        rox_core::pattern::parse(text).expect("the test pattern parses")
    }

    #[test]
    fn the_default_pattern_files_a_song_under_its_station() {
        assert_eq!(
            placed(
                &CaptureSettings::default().parsed_pattern(),
                "Burial",
                "Archangel"
            ),
            "Noise FM/Burial - Archangel"
        );
    }

    #[test]
    fn a_pattern_without_a_slash_stays_flat() {
        assert_eq!(
            placed(&pattern("%artist% - %title%"), "Burial", "Archangel"),
            "Burial - Archangel"
        );
    }

    #[test]
    fn a_pattern_three_deep_digs_three_folders() {
        assert_eq!(
            placed(
                &pattern("%station%/%genre%/%date%/%title%"),
                "Burial",
                "Archangel"
            ),
            "Noise FM/Electronic/2026-09-18/Archangel"
        );
    }

    #[test]
    fn a_station_that_sends_no_artist_leaves_no_separator_behind() {
        assert_eq!(
            placed(&CaptureSettings::default().parsed_pattern(), "", "Xtal"),
            "Noise FM/Xtal"
        );
        assert_eq!(placed(&pattern("%artist% - %title%"), "", "Xtal"), "Xtal");
    }

    #[test]
    fn a_pattern_that_does_not_parse_falls_back_to_the_default() {
        let settings = CaptureSettings {
            pattern: "%tittle%".into(),
            ..CaptureSettings::default()
        };

        assert_eq!(
            placed(&settings.parsed_pattern(), "Burial", "Archangel"),
            "Noise FM/Burial - Archangel"
        );
    }

    #[test]
    fn a_name_already_taken_counts_upwards() {
        let folder = Path::new("/captures");
        let taken = |path: &Path| {
            matches!(
                path.to_str(),
                Some("/captures/Burial - Archangel.mp3")
                    | Some("/captures/Burial - Archangel (2).mp3")
            )
        };

        assert_eq!(
            free_path(folder, "Burial - Archangel", "mp3", &taken),
            Some(PathBuf::from("/captures/Burial - Archangel (3).mp3"))
        );
        assert_eq!(
            free_path(folder, "Autechre - Rae", "mp3", &taken),
            Some(PathBuf::from("/captures/Autechre - Rae.mp3"))
        );
    }

    #[test]
    fn the_tags_name_the_station_as_the_album() {
        let take = Take {
            title: title("Burial", "Archangel"),
            bytes: Vec::new(),
        };
        let station = Station {
            url: "https://example.org/stream".into(),
            name: "NTS 1".into(),
            genre: "Electronic".into(),
            source: "Radio".into(),
            ext: "mp3",
        };

        let fields: Vec<(Field, Option<String>)> = tags_for(&take, &station, "")
            .into_iter()
            .map(|c| (c.field, c.value))
            .collect();

        assert_eq!(
            fields,
            vec![
                (Field::Title, Some("Archangel".into())),
                (Field::Artist, Some("Burial".into())),
                (Field::Genre, Some("Electronic".into())),
                (
                    Field::Comment,
                    Some("NTS 1 (https://example.org/stream)".into())
                ),
            ]
        );
    }

    #[test]
    fn the_album_is_the_settings_call() {
        let take = Take {
            title: title("Burial", "Archangel"),
            bytes: Vec::new(),
        };
        let station = Station {
            url: "https://example.org/stream".into(),
            name: "NTS 1".into(),
            genre: "Electronic".into(),
            source: "Radio".into(),
            ext: "mp3",
        };
        let fields = fields_for(&take.title, &station, &now());
        let album_of = |setting: &str| {
            tags_for(&take, &station, &album_for(setting, &fields))
                .into_iter()
                .find(|c| c.field == Field::Album)
                .and_then(|c| c.value)
        };

        assert_eq!(album_of(""), None);
        assert_eq!(album_of("  "), None);
        assert_eq!(album_of("Singles"), Some("Singles".into()));
        assert_eq!(album_of("%station%"), Some("NTS 1".into()));
        assert_eq!(album_of("Radio: %station%"), Some("Radio: NTS 1".into()));
        assert_eq!(album_of("100% Radio"), Some("100% Radio".into()));
    }

    #[test]
    fn an_empty_station_header_writes_no_empty_tag() {
        let take = Take {
            title: title("", "Untitled Broadcast"),
            bytes: Vec::new(),
        };
        let station = Station {
            url: String::new(),
            name: String::new(),
            genre: String::new(),
            source: String::new(),
            ext: "mp3",
        };

        assert_eq!(
            tags_for(&take, &station, "").len(),
            1,
            "the title, and nothing else"
        );
    }
}
