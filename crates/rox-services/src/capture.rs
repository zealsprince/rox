//! Saving songs off the air. A station's bytes are already going past the
//! transport in the container the station encodes in, and its in-band
//! titles say where one song stops and the next starts, so a song heard
//! from one title change to the next can land on disk with no decoder and
//! no encoder in the way. The file is the station's own frames, byte for
//! byte.
//!
//! The rule this exists to enforce is start to finish. A capture counts
//! only when it began at a boundary, ended at the next one, ran long
//! enough to be a song rather than a jingle, and had no reconnect in the
//! middle. Everything else is thrown away, which includes the very first
//! capture of a connection: a station announces what is already playing
//! the moment you tune in, so the song under that first title was joined
//! halfway through.
//!
//! Where the boundary sits is the honest limitation. Stations flip the
//! title a few seconds either side of the audio actually switching, so a
//! capture routinely carries the tail of the song before it or the head of
//! the one after. Trimming that needs a decoder-side silence or energy
//! detector, which is not in this round; the settings row says so, and the
//! files are written as they came.
//!
//! What a saved song is called is the renamer's pattern language, run
//! through [`rox_core::pattern`] over the only values a broadcast has: the
//! artist and title the air announced, the station, its genre, and the day
//! it was heard. A "/" in the pattern makes a folder under the capture
//! folder, which is what keeps an evening of radio from piling up flat.
//!
//! A saved song gets a cover beside it, under the same name with an image
//! extension, from the same providers the now-playing art searches. The
//! picture is a guess off two strings a station sent, the same guess the
//! backdrop is already showing while the song is on air, and a wrong guess
//! on a file kept on disk is worth more caution than one behind a blur: it
//! has to clear the art matcher's bar before it's written at all.
//!
//! Nothing here touches the decode thread. [`rox_playback::icy`] copies
//! bytes into a channel and that is the whole of its job; the buffer, the
//! rule, the tag write and the scan all run here, on the player's pump and
//! the background executor.

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

/// How long a song has to run before it's worth keeping. The scrobbler's
/// own floor (`MIN_TRACK_SECS` in `lastfm.rs`), for the same reason: under
/// thirty seconds is a station ident, a trailer or a bad boundary, not a
/// play.
const MIN_SECS: f64 = 30.0;

/// The bitrate assumed for a station that states none, so the length rule
/// still has a number to work from. Well under what most stations run, so
/// the guess errs towards keeping a song rather than dropping it.
const ASSUMED_KBPS: u32 = 128;

/// The hard ceiling on one capture, whatever the settings say. A station
/// whose titles stop moving would otherwise buffer the rest of the
/// evening; sixty-four megabytes is around half an hour at 320 kbps.
const CAP_BYTES: usize = 64 * 1024 * 1024;

/// How long a capture may run, in seconds, before it's abandoned: the live
/// buffer length from settings. The two are one idea from the listener's
/// side. The buffer is how far back a station can be rewound, and a song
/// that never ends inside it (a podcast, a mix, a station that stopped
/// announcing) isn't a song to save. `apply` keeps it current.
static CEILING_SECS: AtomicU32 = AtomicU32::new(settings::DEFAULT_LIVE_BUFFER_SECS);

/// How many counted names a collision walks through before giving up. A
/// station replaying a song is ordinary, a thousand copies of it is not.
const MAX_COPIES: u32 = 999;

/// One finished song: the title it was announced under, and every byte the
/// station sent between the two boundaries that bracket it.
pub struct Take {
    pub title: IcyTitle,
    pub bytes: Vec<u8>,
}

/// The song currently being written down.
struct InFlight {
    title: IcyTitle,
    bytes: Vec<u8>,
}

/// The start-to-finish rule with no app and no disk around it: events in,
/// finished songs out.
pub struct Tape {
    current: Option<InFlight>,
    /// Whether the next boundary is the announcement a station makes when
    /// you tune in rather than a song starting. True at the start and
    /// again after every drop, since a reconnect rejoins mid-song exactly
    /// the way the first connect did.
    joined: bool,
    /// How many bytes a song has to reach to count as heard, the thirty
    /// seconds turned into bytes at the station's own bitrate.
    min_bytes: usize,
    /// The station's stated bitrate, kept rather than a byte ceiling
    /// computed from it once: the ceiling follows the live buffer setting,
    /// and that setting moves while a station plays, so it's read at each
    /// check instead of frozen at connect.
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
    /// Take the station's stated bitrate as the length yardstick. A capture
    /// has no decoder behind it, so byte count at the advertised rate is
    /// the only duration it can know.
    fn follow_bitrate(&mut self, kbps: u32) {
        self.min_bytes = min_bytes_at(kbps);
        self.kbps = kbps;
    }

    /// Feed one event, and answer with a song if that event finished one.
    fn feed(&mut self, event: CaptureEvent) -> Option<Take> {
        match event {
            CaptureEvent::Boundary(title) => {
                // The first title on a connection names what was already
                // playing when we arrived, so it opens nothing.
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
                // Bytes with no song open are the ones before the first
                // boundary, or the ones after a capture was abandoned.
                let current = self.current.as_mut()?;

                // Past the ceiling this isn't a song, it's a broadcast that
                // never announced its end. Drop it and wait for a boundary,
                // and say so: a buffer set shorter than the songs on air
                // otherwise looks like capture doing nothing at all.
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
                // The connection went away. What was in flight lost its
                // middle, and whatever the reconnect announces first is a
                // song already underway.
                self.current = None;
                self.joined = true;
                None
            }
        }
    }
}

/// What a capture's tags come from: the row it played under and the
/// headers the stream answered the connect with.
#[derive(Clone, PartialEq)]
struct Station {
    /// The stream URL, which is both the station row's path and the
    /// comment the capture carries.
    url: String,
    name: String,
    genre: String,
    /// The provider the station belongs to, as [`source_label`] spells it.
    source: String,
    /// The container the station encodes in, as a file extension.
    ext: &'static str,
}

/// The provider a stream belongs to, as a folder name: the source id's
/// kind with its first letter up, so `radio` files as "Radio" and a
/// `subsonic:<server>` as "Subsonic". Read off the id rather than a
/// table, so a source that doesn't exist yet names its own folder without
/// anyone coming back here.
pub fn source_label(source: &str) -> String {
    let kind = source.split(':').next().unwrap_or(source).trim();
    let mut chars = kind.chars();

    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What a capture pattern fills. Every placeholder in the app-wide
/// vocabulary parses here, so a pattern carried over from the rename
/// dialog still works. What the air actually gives you is an artist, a
/// title, the station, its genre, the container and the clock;
/// everything else parses and renders as nothing.
#[derive(Clone, PartialEq)]
pub enum CaptureField {
    Artist,
    Title,
    /// The station's name. %album% spells the same thing, since a station
    /// is the only release a song off the air belongs to.
    Station,
    /// The kind of source the stream came from, "Radio" today. Its own
    /// field so a pattern can file by provider, and a source that doesn't
    /// exist yet lands in a folder of its own the day it ships.
    Source,
    Genre,
    /// The container the stream sends, "MP3". As much of a format as a
    /// broadcast states: the bitrate it claims in its headers is a
    /// station-wide number rather than this song's.
    Format,
    /// The year the capture landed, off the same clock as the day.
    Year,
    /// The day the capture landed, as `YYYY-MM-DD`.
    Date,
    /// A placeholder a broadcast can't fill: an album artist, a track or
    /// disc number, a comment. It parses and renders nothing, taking its
    /// separator with it, rather than refusing a pattern the rename
    /// dialog would have accepted.
    Unfilled,
}

impl PatternField for CaptureField {
    fn from_name(name: Name) -> Option<Self> {
        Some(match name {
            Name::Artist => CaptureField::Artist,
            Name::Title => CaptureField::Title,
            // A station is the only release a song off the air belongs
            // to, so the album is the station.
            Name::Album | Name::Station => CaptureField::Station,
            Name::Source => CaptureField::Source,
            Name::Genre => CaptureField::Genre,
            Name::Format => CaptureField::Format,
            Name::Year => CaptureField::Year,
            // The renamer reads %date% as the release year. A stream has
            // no release, so here it's the day the song was heard, which
            // is the only date a capture has.
            Name::Date => CaptureField::Date,
            Name::AlbumArtist | Name::Track | Name::Disc | Name::Comment => CaptureField::Unfilled,
            Name::Skip => return None,
        })
    }

    fn fallback(&self) -> &'static str {
        match self {
            // The artist is allowed to vanish. Plenty of stations send a
            // single unsplittable title, and "Unknown Artist - Song" on
            // every one of them is a worse name than "Song".
            CaptureField::Artist | CaptureField::Unfilled => "",

            // What a nameless capture has been called since before there
            // was a pattern.
            CaptureField::Title => "Capture",

            CaptureField::Station => "Unknown Station",
            CaptureField::Source => "Unknown Source",
            CaptureField::Genre => "Unknown Genre",
            CaptureField::Format => "Unknown Format",

            // Both read off the clock at write time, so nothing reaches
            // these from a real capture. Kept total rather than
            // panicking.
            CaptureField::Year => "Unknown Year",
            CaptureField::Date => "Unknown Date",
        }
    }
}

/// When a capture landed, in the two shapes a pattern can ask for.
struct Stamp {
    day: String,
    year: String,
}

/// The clock, read once per saved song. Local rather than UTC: the day a
/// listener files a song under is the day they heard it.
fn now() -> Stamp {
    let now = chrono::Local::now();

    Stamp {
        day: now.format("%Y-%m-%d").to_string(),
        year: now.format("%Y").to_string(),
    }
}

/// The capture service: one per player, holding the in-flight song and
/// writing the finished ones out. Headless like the rest of the services
/// here, and it refers to no panel.
pub struct Capture {
    events: Receiver<CaptureEvent>,
    tape: Tape,
    /// The station the tape belongs to, read off the player as it plays so
    /// a finished song is tagged with what played it rather than with
    /// whatever is on by the time the write lands.
    station: Option<Station>,
    library: Entity<Library>,
    _player_changed: Subscription,
}

impl Capture {
    pub fn new(player: &Entity<Player>, library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // Unbounded on purpose. The far end is the decode thread copying a
        // station's bytes at the station's own bitrate, a few tens of
        // kilobytes a second, and this end drains on the sixteen
        // millisecond pump, so the queue is bounded by how far behind the
        // UI thread is rather than by anything the stream does. A bounded
        // channel would answer a full queue by dropping a batch, and a
        // capture with a hole in the middle is worse than no capture.
        let (tx, events) = std::sync::mpsc::channel();
        rox_playback::icy::tee_to(sink(tx));
        apply();

        // The same pump clock the scrobbler and the live-title service
        // ride: every tick while a session runs.
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
        // Who is playing, read before the drain: a song finishing on this
        // tick belongs to the station that was on when its bytes arrived.
        self.follow(player, cx);

        // Drain first, write second. The writes take the app mutably, and
        // the receiver is borrowed for as long as the loop runs.
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

    /// Note which station is playing, and take its bitrate as the length
    /// yardstick. A local file leaves the last station in place, which
    /// costs nothing: the tape only ever fills from a stream.
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

    /// Put one finished song on disk and into the library. The write is a
    /// few megabytes, a tag parse and a cover lookup, so it goes to the
    /// background executor; the reindex comes back to the UI thread
    /// because the catalog lives there.
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

                    // Before the reindex rather than after: the panels
                    // repaint on the library event, and a thumbnail asked
                    // for before the picture is beside it caches as a
                    // definitive no-art answer.
                    save_cover(&path, &take.title);

                    Ok(path)
                })
                .await;

            match written {
                // Explicitly, rather than leaving it to the root watcher:
                // the capture folder is not required to be a library root,
                // and the watcher drops events under load anyway.
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

/// Point the tee at the current settings. Startup builds the service,
/// which calls this once; the settings row calls it again to make a change
/// live, the same shape the broadcast sink's switch has.
pub fn apply() {
    let settings = Settings::load();
    rox_playback::icy::set_capturing(settings.capture.enabled);
    follow_live_buffer(settings.live_buffer_secs);
}

/// The live buffer moved. The player calls this from its setter, so a
/// slider change reaches the ceiling while a station plays, rather than
/// waiting for the next launch or the capture switch.
pub fn follow_live_buffer(secs: u32) {
    CEILING_SECS.store(settings::clamp_live_buffer_secs(secs), Ordering::Relaxed);
}

/// The sink the transport feeds. A channel send and nothing else: this
/// runs inside the decode thread's read.
fn sink(tx: Sender<CaptureEvent>) -> CaptureSink {
    // The service outlives every reader that will ever use this, so a
    // disconnected send means the app is going down and there is nothing
    // to do about it.
    Arc::new(move |event| {
        let _ = tx.send(event);
    })
}

/// How many bytes of a stream at `kbps` make [`MIN_SECS`] of audio.
fn min_bytes_at(kbps: u32) -> usize {
    bytes_at(kbps, MIN_SECS)
}

/// How many bytes of a stream at `kbps` make the live buffer's worth of
/// audio, capped at [`CAP_BYTES`].
fn max_bytes_at(kbps: u32) -> usize {
    bytes_at(kbps, f64::from(CEILING_SECS.load(Ordering::Relaxed))).min(CAP_BYTES)
}

fn bytes_at(kbps: u32, secs: f64) -> usize {
    let kbps = if kbps == 0 { ASSUMED_KBPS } else { kbps };

    (f64::from(kbps) * 1000.0 / 8.0 * secs) as usize
}

/// `<Artist> - <Title>` as a filename, or the title alone for the stations
/// that send one unsplittable field.
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

/// The first free name in `folder`, counted upwards past a collision. A
/// station replaying a song is the ordinary case, so this is a path the
/// feature takes often rather than an edge.
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

/// The tags a capture carries. The comment names the station and its
/// stream URL, because that's the only provenance a song off the air has,
/// and the comment is where provenance belongs. The album is whatever the
/// setting rendered, and nothing when it's blank: a song off the air has
/// no release, and an album field full of station names pollutes every
/// album view it lands in.
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

/// The comment line: the station's name and its stream URL, or whichever
/// of the two it gave us.
fn provenance(station: &Station) -> String {
    match (station.name.trim(), station.url.trim()) {
        ("", url) => url.to_string(),
        (name, "") => name.to_string(),
        (name, url) => format!("{name} ({url})"),
    }
}

/// The album tag the setting asks for, rendered with the same vocabulary a
/// name pattern gets, so `%station%` works there too. Blank stays blank.
/// Text that isn't a pattern (a stray `%`) or won't render is written as
/// typed rather than dropped: "Singles" should never fail to be "Singles".
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

/// The values a capture pattern renders from: what the air announced,
/// what the station calls itself, and when the song landed.
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

/// Where a capture sits under the capture folder: the pattern rendered,
/// every segment of it folded into something a filesystem will take. A
/// pattern that renders nothing at all (an untitled song under a folder
/// the station left unnamed) falls back to the flat name captures carried
/// before there was a pattern.
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

/// Every segment of a rendered path folded into something a filesystem
/// will take. The values a pattern put there went through this on the way
/// in; the literal text the pattern wrote around them did not.
fn fold_segments(path: &Path) -> PathBuf {
    path.iter()
        .map(|segment| safe_file_stem(&segment.to_string_lossy(), "Capture"))
        .collect()
}

/// What a pattern is previewed against in the settings row that types it:
/// the station on the air, or a stand-in when nothing is playing.
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
    /// The station playing right now, when one is. Better than the
    /// stand-in, because what the row is really being asked is how this
    /// station's titles come out.
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

/// What `pattern` would call the sample, or what's wrong with it. Runs
/// the same render a saved song takes, so the line under the settings
/// input can't drift from what lands on disk.
pub fn preview(pattern: &str, sample: &Sample) -> Result<String, String> {
    let parsed = rox_core::pattern::parse::<CaptureField>(pattern)?;
    let station = Station {
        url: String::new(),
        name: sample.station.clone(),
        genre: sample.genre.clone(),
        source: sample.source.clone(),
        // The container is appended below rather than rendered; a pattern
        // never names it.
        ext: "",
    };

    let fields = fields_for(&sample.title, &station, &now());
    let rendered = fold_segments(&parsed.render(&fields)?);

    Ok(format!("{}.{}", rendered.display(), sample.ext))
}

/// Write one capture out: the bytes, then the tags through the writer's
/// atomic layer (ADR 4, which is the only sanctioned way tags reach a
/// file). Blocking; the caller runs it on the background executor.
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

    // The pattern's folders, dug under the capture folder. Only the name
    // on the end counts through a collision; the folders above it are
    // shared by design.
    let dir = match relative.parent() {
        Some(parent) => folder.join(parent),
        None => folder.to_path_buf(),
    };
    let stem = relative.file_name().unwrap_or_default().to_string_lossy();

    std::fs::create_dir_all(&dir).map_err(|e| format!("making {}: {e}", dir.display()))?;

    let path = free_path(&dir, &stem, station.ext, &|path| path.exists())
        .ok_or_else(|| format!("no free name left for {stem}"))?;

    // Through a sibling temp file and a rename, so a watcher over the
    // folder never sees a half-written capture and indexes it as a
    // truncated track.
    let part = path.with_extension(format!("{}.part", station.ext));
    std::fs::write(&part, &take.bytes).map_err(|e| format!("writing {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("naming {}: {e}", path.display()))?;

    // A container the tagger can't parse still holds playable audio, and a
    // mid-stream Ogg capture is exactly that: it has no header pages, so
    // lofty has nothing to open. Keep the file and say what was lost.
    if let Err(e) = writer::commit(&path, &tags_for(take, station, &album)) {
        log::warn!("capture: {} saved untagged: {e}", path.display());
    }

    Ok(path)
}

/// The song's cover next to the file that holds it, under the same name.
/// A station's evening lands in one folder, so a shared cover.jpg there
/// would be the wrong picture for all but one song; the per-track name is
/// what [`rox_library::art`] ranks first for a track, and it travels with
/// the song when it's moved out of the capture folder.
///
/// The same lookup the backdrop runs for the song on air, and that one has
/// already searched these two names by the time the song ends, so the
/// search half comes back off the provider session cache and only the
/// download is new. Silent on every way it comes to nothing: the picture
/// rides along with the audio and is never the reason a capture failed.
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

    // Through a sibling temp file and a rename, the audio's own move: a
    // reader that catches a half-written picture caches the track as
    // having art that will not decode.
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

    /// Two tests move the ceiling, which is one global, so they take
    /// turns rather than read each other's value mid-assertion.
    static CEILING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn title(artist: &str, song: &str) -> IcyTitle {
        IcyTitle {
            artist: artist.into(),
            title: song.into(),
        }
    }

    /// A tape that keeps anything past ten bytes, so the tests are about
    /// the boundaries rather than about arithmetic.
    fn tape() -> Tape {
        Tape {
            min_bytes: 10,
            ..Tape::default()
        }
    }

    #[test]
    fn the_song_playing_when_we_tuned_in_is_not_saved() {
        let mut tape = tape();

        // The announcement on connect, then the rest of a song we joined
        // halfway through, then the real first boundary.
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

        // Long enough to keep, had it not been cut in half.
        assert!(tape.feed(CaptureEvent::Boundary(title("C", "3"))).is_none());

        // And the boundary after a drop is another tune-in, so the song it
        // names is underway too.
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

    /// The ceiling is the live buffer at the station's rate: ten minutes
    /// at 128 kbps is 9.6 MB, and a station stating nothing is measured at
    /// the assumed rate. The hard cap still wins over a huge buffer.
    /// A song already taping is judged against the buffer as it is now,
    /// not as it was when the station connected.
    #[test]
    fn a_longer_buffer_reaches_a_song_in_flight() {
        let _guard = CEILING_LOCK.lock().unwrap();
        CEILING_SECS.store(60, Ordering::Relaxed);
        let mut tape = Tape::default();
        tape.follow_bitrate(128);
        tape.feed(CaptureEvent::Boundary(title("A", "First")));
        tape.feed(CaptureEvent::Boundary(title("B", "Second")));

        // Ninety seconds at 128 kbps, over a one minute buffer.
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
        // A station that states nothing is measured at the assumed rate.
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

    /// %source% is the provider, spelled off the id so a source nobody has
    /// written yet already has a folder name.
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

    /// Where one capture lands under the capture folder, as a relative
    /// path, which is the whole of what the pattern decides.
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

    /// A pattern nobody can render is a settings file somebody edited by
    /// hand, or a placeholder retired out from under one. Captures keep
    /// landing, under the default.
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

        // No album by default: the station is provenance, so it goes in
        // the comment beside the URL and never into the album field.
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

    /// The album is whatever the setting says: a plain word as typed, a
    /// pattern rendered with the station's own vocabulary, and nothing when
    /// it's blank. Text that isn't a pattern is still written as typed.
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
