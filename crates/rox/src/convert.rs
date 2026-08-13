//! Converting tracks to another format by spawning ffmpeg.
//!
//! rox decodes plenty on its own, but it encodes nothing: writing a FLAC or
//! an Opus file means an encoder, and the one every machine either already
//! has or can install in a line is ffmpeg. So this is the app's one external
//! process. It stays a feature that only exists when the binary does:
//! [`available`] probes once per session and every surface that offers a
//! conversion is gated on it, so a machine without ffmpeg never sees a
//! "Convert..." it can't follow.
//!
//! The shape of the run is [`crate::replaygain_job`]'s: an app-global
//! `Arc<Progress>` the tasks window polls, blocking work on the background
//! executor, a bounded pool over a cursor. The pool is smaller than a
//! measuring pass's because a worker here is a whole ffmpeg process rather
//! than a decode loop, and four of those already own the machine.
//!
//! The interesting case is a cue track. It has no file of its own, only a
//! span inside an image the whole disc shares, so converting one means
//! trimming: `-ss`/`-to` as input options, which is what makes the seek land
//! on the frame rather than near it. The image's tags describe the album
//! rather than that track, so a span drops them and writes title, artist,
//! album and track number from the library row instead. That's what turns a
//! rip into a standalone file, and it's the one thing this does that a
//! shell loop over ffmpeg doesn't.
//!
//! Nothing here ever overwrites. A destination that exists is reported as
//! skipped and left exactly as it was; there is no flag anywhere that turns
//! that off, because the alternative is a typo in a pattern eating a
//! library.
//!
//! Past the five presets there's [`Custom`], which is an extension and a
//! line of ffmpeg arguments someone typed. Two things keep that from being
//! a hole in everything above it. The arguments are tokenized and handed
//! over as a vector, never a shell, and [`parse_args`] refuses outright
//! anything this module owns rather than quietly dropping it: the input,
//! the container, the overwrite flags, the destination. And a combination
//! doesn't run until [`check`] has encoded a tenth of a second of silence
//! with it, so "Unknown encoder" arrives in the dialog rather than as a
//! hundred failed files.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{App, Global};

use rox_core::settings::Settings;
use rox_library::writer::Field;

use crate::tags::guess;

/// The pattern a first run names files with: flat in the destination, one
/// file per track, which is what someone filling a phone or a USB stick is
/// after.
pub const DEFAULT_PATTERN: &str = "%artist% - %title%";

/// The pattern behind the mirror toggle: the library's own folder shape,
/// for a copy of a collection rather than a handful of files.
pub const MIRROR_PATTERN: &str = "%albumartist%/%album%/%track% - %title%";

/// The most ffmpeg processes a run keeps going at once. Each one is a
/// whole encoder, so this is deliberately below the worker counts the
/// analysis passes use: past four the machine is the job.
const MAX_WORKERS: usize = 4;

/// How often a worker looks up from a running ffmpeg to see whether the run
/// has been cancelled. Short enough that Stop feels immediate, long enough
/// that polling costs nothing next to encoding.
const POLL: Duration = Duration::from_millis(100);

/// How much of a failed ffmpeg's stderr is worth keeping. The last few
/// lines carry the reason; everything before them is banner and progress.
const STDERR_TAIL: usize = 400;

/// Windows' flag for a child that gets no console. Without it every
/// conversion pops a black window in front of the app.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// An ffmpeg invocation, with the window suppressed and the pipes settled.
/// Every spawn in this module goes through here.
fn command(binary: &str) -> Command {
    let mut command = Command::new(binary);
    command
        // ffmpeg reads stdin for its interactive keys, and a child sharing
        // a terminal with the app would eat what's typed at it.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Which ffmpeg to spawn: the path from settings when one is set, the one
/// on PATH otherwise.
pub fn binary() -> String {
    let custom = Settings::load().convert.ffmpeg;
    let custom = custom.trim();
    if custom.is_empty() {
        "ffmpeg".to_string()
    } else {
        custom.to_string()
    }
}

/// What each binary answered when it was asked its version, so the probe
/// costs one spawn per session rather than one per menu that opens. Keyed
/// by the binary, so pointing the setting at another one re-probes instead
/// of trusting the first answer forever.
static PROBED: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

/// Whether this machine can convert at all. Every surface that offers a
/// conversion asks first: with no ffmpeg there is no menu item, no dialog
/// and nothing in settings search, which is a better answer than a button
/// that explains itself only after it's pressed.
pub fn available() -> bool {
    let binary = binary();
    let probed = PROBED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&known) = probed.lock().unwrap().get(&binary) {
        return known;
    }
    let found = probe(&binary);
    probed.lock().unwrap().insert(binary, found);
    found
}

/// Ask a binary its version. Anything other than a clean exit reads as not
/// there: a path that doesn't resolve, a file that isn't executable, and
/// something that isn't ffmpeg all fail the same way and all mean the same
/// thing here.
fn probe(binary: &str) -> bool {
    version(binary).is_ok()
}

/// The version a binary answers with, or why there wasn't one. The boolean
/// probe folds every failure into "not there"; the settings test button
/// wants the distinction back, so this keeps what the spawn said.
fn version(binary: &str) -> Result<String, String> {
    let mut ask = command(binary);
    ask.stdout(Stdio::piped());
    match ask.arg("-version").output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // The first line is "ffmpeg version N.N ..." with a copyright
            // notice trailing it; the notice says nothing the callout needs.
            let line = stdout
                .lines()
                .next()
                .unwrap_or("")
                .split(" Copyright")
                .next()
                .unwrap_or("")
                .trim();
            if line.is_empty() {
                Ok(format!("{binary} answered"))
            } else {
                Ok(line.to_string())
            }
        }
        Ok(out) => Err(tail(&String::from_utf8_lossy(&out.stderr))),
        Err(e) => Err(e.to_string()),
    }
}

/// The settings test button's probe: fresh every press rather than served
/// from the session cache, because the point of pressing it is that the
/// world may have changed. The answer lands in the cache too, so a pass
/// flips every Convert surface on without a restart.
pub fn test() -> Result<String, String> {
    let binary = binary();
    let answer = version(&binary);
    PROBED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap()
        .insert(binary, answer.is_ok());
    answer
}

/// What a conversion produces, as a fixed table. No format knobs: the point
/// of a preset is that the choice is "a good FLAC" rather than a compression
/// level someone has to have an opinion about.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Preset {
    /// Lossless and the one a library keeps, so it's where a first run
    /// starts.
    #[default]
    Flac,
    Mp3320,
    Mp3V0,
    Opus192,
    Wav,
}

impl Preset {
    /// Every preset, in the order the dropdown lists them: lossless first,
    /// then the lossy ones by how common they are, then WAV, which is for
    /// handing audio to something that won't take anything else.
    pub const ALL: [Preset; 5] = [
        Preset::Flac,
        Preset::Mp3320,
        Preset::Mp3V0,
        Preset::Opus192,
        Preset::Wav,
    ];

    /// The name the settings file remembers a preset by, so a build that
    /// adds one doesn't renumber the rest.
    pub fn key(self) -> &'static str {
        match self {
            Preset::Flac => "flac",
            Preset::Mp3320 => "mp3-320",
            Preset::Mp3V0 => "mp3-v0",
            Preset::Opus192 => "opus-192",
            Preset::Wav => "wav",
        }
    }

    /// The preset a key names, or None for one this build doesn't know.
    pub fn from_key(key: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::Flac => "FLAC",
            Preset::Mp3320 => "MP3 320 kbps",
            Preset::Mp3V0 => "MP3 V0",
            Preset::Opus192 => "Opus 192 kbps",
            Preset::Wav => "WAV",
        }
    }

    /// The extension the output takes, which is also what tells ffmpeg
    /// which container to write.
    pub fn ext(self) -> &'static str {
        match self {
            Preset::Flac => "flac",
            Preset::Mp3320 | Preset::Mp3V0 => "mp3",
            Preset::Opus192 => "opus",
            Preset::Wav => "wav",
        }
    }

    /// The encoder and its one setting.
    fn codec(self) -> &'static [&'static str] {
        match self {
            Preset::Flac => &["-c:a", "flac", "-compression_level", "8"],
            Preset::Mp3320 => &["-c:a", "libmp3lame", "-b:a", "320k"],
            Preset::Mp3V0 => &["-c:a", "libmp3lame", "-q:a", "0"],
            Preset::Opus192 => &["-c:a", "libopus", "-b:a", "192k"],
            Preset::Wav => &["-c:a", "pcm_s16le"],
        }
    }

    /// Whether the container can carry the source's embedded cover art.
    /// FLAC and MP3 hold a picture block; Opus in an Ogg stream and WAV
    /// have nowhere to put one, so those drop it rather than failing the
    /// encode over it.
    fn keeps_art(self) -> bool {
        matches!(self, Preset::Flac | Preset::Mp3320 | Preset::Mp3V0)
    }
}

/// A format the table doesn't have: the container its extension names, and
/// the ffmpeg output arguments someone typed for it, already split into
/// tokens. Built through [`Custom::parse`], which is the only way in and
/// the place every refusal happens.
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Custom {
    pub ext: String,
    pub args: Vec<String>,
}

impl Custom {
    /// A custom format out of the dialog's two inputs, or the sentence
    /// saying why it isn't one.
    pub fn parse(ext: &str, args: &str) -> Result<Custom, String> {
        let ext = ext.trim().trim_start_matches('.').trim();
        if ext.is_empty() {
            return Err("The extension is what picks the container, so it needs one".into());
        }
        if !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "\"{ext}\" isn't a container name; letters and digits, no dot"
            ));
        }
        Ok(Custom {
            ext: ext.to_ascii_lowercase(),
            args: parse_args(args)?,
        })
    }
}

/// Flags this module owns, and what to say when one turns up in a custom
/// argument list. Refused rather than stripped: someone who typed `-y`
/// meant it, and a silent removal would leave them believing the opposite
/// of what runs.
const OWNED_FLAGS: [(&str, &str); 5] = [
    (
        "-y",
        "Nothing here overwrites, so -y isn't available; a destination that exists is skipped",
    ),
    ("-n", "-n is already on every run"),
    (
        "-i",
        "The input is the track you picked, so -i isn't yours to set",
    ),
    (
        "-f",
        "The extension picks the container, so -f isn't yours to set",
    ),
    (
        "-attach",
        "-attach reads a file of its own, which this doesn't allow",
    ),
];

/// Whether a token reads as a file name rather than a value. Slashes and a
/// short alphabetic tail after a dot are what a path looks like; a value
/// with an `=` in it is a setting however many dots it carries, which is
/// what keeps `-af volume=0.5` out of this.
fn looks_like_a_file(token: &str) -> bool {
    if token.contains('=') {
        return false;
    }
    if token.contains('/') || token.contains('\\') {
        return true;
    }
    match token.rsplit_once('.') {
        Some((head, tail)) => {
            !head.is_empty()
                && !tail.is_empty()
                && tail.len() <= 5
                && tail.chars().all(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// Split a line of ffmpeg arguments into the vector that gets spawned, or
/// say why it can't be one.
///
/// The split is plain whitespace and there is no quoting: a value with a
/// space in it can't be written here, which is a real limit and a cheap
/// one next to parsing shell syntax nobody is running.
///
/// What comes back never reaches a shell, so the refusals aren't about
/// escaping. They're about the parts of the command line [`args`] owns:
/// the flags in [`OWNED_FLAGS`], and any bare token that reads as a file,
/// which in this position could only be a second output.
pub fn parse_args(text: &str) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut after_flag = false;
    for token in text.split_whitespace() {
        if let Some((_, reason)) = OWNED_FLAGS.iter().find(|(flag, _)| *flag == token) {
            return Err((*reason).to_owned());
        }
        // A negative number is a value, not a flag: -1 after -map_metadata
        // is the clearest case and it reads as a flag on a naive check.
        let flag = token.starts_with('-')
            && token.len() > 1
            && !token[1..].starts_with(|c: char| c.is_ascii_digit());
        if !flag && looks_like_a_file(token) {
            return Err(format!(
                "\"{token}\" names a file; the destination comes from the folder and the pattern"
            ));
        }
        if !flag && !after_flag {
            return Err(format!("\"{token}\" isn't a flag or a value for one"));
        }
        after_flag = flag;
        tokens.push(token.to_owned());
    }
    Ok(tokens)
}

/// What a run encodes to: one of the five, or someone's own.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Format {
    Preset(Preset),
    Custom(Custom),
}

impl Default for Format {
    fn default() -> Self {
        Format::Preset(Preset::default())
    }
}

impl Format {
    /// What the settings file calls a custom format. Presets write their
    /// own keys and [`Preset::from_key`] only answers to those five, so
    /// nothing collides with this.
    pub const CUSTOM_KEY: &'static str = "custom";

    /// The name the settings file remembers this by.
    pub fn key(&self) -> &str {
        match self {
            Format::Preset(preset) => preset.key(),
            Format::Custom(_) => Format::CUSTOM_KEY,
        }
    }

    /// The extension the output takes, which is also what tells ffmpeg
    /// which container to write.
    pub fn ext(&self) -> &str {
        match self {
            Format::Preset(preset) => preset.ext(),
            Format::Custom(custom) => &custom.ext,
        }
    }

    /// The encoder arguments, which sit between what this module owns and
    /// the destination.
    fn encoder(&self) -> Vec<String> {
        match self {
            Format::Preset(preset) => preset.codec().iter().map(|a| (*a).to_owned()).collect(),
            Format::Custom(custom) => custom.args.clone(),
        }
    }

    /// Whether an embedded cover rides along. Known per preset, and no for
    /// a custom: nothing here knows what an arbitrary container does with
    /// an attached picture, and mapping one into a muxer that won't take it
    /// fails the whole encode rather than just losing the picture.
    fn keeps_art(&self) -> bool {
        match self {
            Format::Preset(preset) => preset.keeps_art(),
            Format::Custom(_) => false,
        }
    }
}

/// How long the check's silence runs. Long enough that every encoder here
/// writes a frame, short enough that the spawn is the cost rather than the
/// encode.
const CHECK_SECONDS: &str = "0.1";

/// What each custom combination was found to be, so reopening the dialog on
/// one that already passed costs nothing. Keyed by the pair itself, so
/// changing either the extension or a single argument is a fresh question.
static CHECKED: OnceLock<Mutex<HashMap<Custom, Result<(), String>>>> = OnceLock::new();

fn checks() -> &'static Mutex<HashMap<Custom, Result<(), String>>> {
    CHECKED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What this session already knows about a custom format, if anything. The
/// dialog asks before it spawns, and shows the answer without a wait.
pub fn checked(custom: &Custom) -> Option<Result<(), String>> {
    checks().lock().unwrap().get(custom).cloned()
}

/// Put a custom format through ffmpeg for real: a tenth of a second of
/// silence, encoded with these arguments into this container, into a temp
/// file that is removed either way. Blocking, so callers run it off the UI
/// thread; the answer is cached against the pair.
///
/// This is the only honest check available. Arguments are ffmpeg's own
/// vocabulary, they change between builds, and whether a container takes an
/// encoder is a question only the binary on this machine can answer.
pub fn check(custom: &Custom) -> Result<(), String> {
    if let Some(known) = checked(custom) {
        return known;
    }
    let answer = run_check(custom, &binary());
    checks()
        .lock()
        .unwrap()
        .insert(custom.clone(), answer.clone());
    answer
}

fn run_check(custom: &Custom, binary: &str) -> Result<(), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dest = std::env::temp_dir().join(format!(
        "rox-convert-check-{}-{stamp}.{}",
        std::process::id(),
        custom.ext
    ));
    let mut args: Vec<String> = [
        "-nostdin",
        "-n",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=44100:cl=stereo",
        "-t",
        CHECK_SECONDS,
        "-vn",
    ]
    .iter()
    .map(|a| (*a).to_owned())
    .collect();
    args.extend(custom.args.iter().cloned());
    args.push(dest.to_string_lossy().into_owned());
    let out = command(binary)
        .args(&args)
        .output()
        .map_err(|e| format!("{binary}: {e}"))?;
    let wrote = dest.is_file();
    let _ = std::fs::remove_file(&dest);
    if !out.status.success() {
        return Err(tail(&String::from_utf8_lossy(&out.stderr)));
    }
    // A clean exit over an empty folder means `-n` refused a name that was
    // somehow taken, which says nothing about the arguments.
    if !wrote {
        return Err("ffmpeg exited clean but wrote nothing".into());
    }
    Ok(())
}

/// The stretch of an image one cue track is. `end_ms` is None on the last
/// track of a sheet, which runs to the file's own end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start_ms: u32,
    pub end_ms: Option<u32>,
}

/// One conversion: a source file, where it lands, and what it is. `span`
/// makes it a trim out of an image rather than a whole file, and `tags` is
/// what a trim writes in place of the image's own metadata.
#[derive(Clone, PartialEq, Debug)]
pub struct Item {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub span: Option<Span>,
    pub tags: Vec<(Field, String)>,
}

/// A duration in ffmpeg's seconds form. Milliseconds are what the sheet
/// carries and three decimals is what they are, so no rounding happens
/// anywhere between the row and the trim.
fn secs(ms: u32) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// A field's value out of a row's tags, empty ones skipped: writing
/// `-metadata artist=` would stamp an empty tag over nothing, which reads
/// worse than the absent tag it replaces.
fn value<'a>(tags: &'a [(Field, String)], field: &Field) -> Option<&'a str> {
    tags.iter()
        .find(|(f, _)| f == field)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// The whole ffmpeg command line for one item, the binary aside. Pure, so
/// what gets spawned is a thing the tests can read.
///
/// `-n` on every invocation, never `-y`: it makes ffmpeg exit rather than
/// touch a file that already exists. Leaving it to the absent `-y` is not
/// enough - with stdin closed, ffmpeg 8 answers its own overwrite prompt and
/// answers yes - and the planner's skip is then the second lock on the same
/// door rather than the only one.
pub fn args(item: &Item, format: &Format) -> Vec<String> {
    let mut args: Vec<String> = ["-nostdin", "-n", "-hide_banner", "-loglevel", "error"]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    // Input options, before -i on purpose: seeking the input is what
    // decodes from the nearest keyframe up to the mark, so the cut lands
    // where the sheet says rather than at the frame ffmpeg happened to be
    // holding. -to reads as an input timestamp in this position, so the
    // pair is exactly the sheet's window.
    if let Some(span) = item.span {
        args.push("-ss".into());
        args.push(secs(span.start_ms));
        if let Some(end) = span.end_ms {
            args.push("-to".into());
            args.push(secs(end));
        }
    }
    args.push("-i".into());
    args.push(item.src.to_string_lossy().into_owned());
    match item.span {
        // A span's metadata is the album's, so none of it comes across:
        // -map_metadata -1 clears the lot and the row's own values go in
        // behind it. Without the -1 ffmpeg copies input metadata by
        // default, and the output would claim to be the whole disc.
        Some(_) => {
            args.push("-vn".into());
            args.push("-map_metadata".into());
            args.push("-1".into());
            for (name, field) in [
                ("title", Field::Title),
                ("artist", Field::Artist),
                ("album", Field::Album),
                ("track", Field::TrackNo),
            ] {
                if let Some(value) = value(&item.tags, &field) {
                    args.push("-metadata".into());
                    args.push(format!("{name}={value}"));
                }
            }
        }
        // A whole file keeps what it says about itself, and its cover with
        // it where the output container can hold one. The picture rides as
        // a video stream, so keeping it means mapping it through rather
        // than -vn, and copying it rather than re-encoding.
        None => {
            if format.keeps_art() {
                args.push("-map".into());
                args.push("0:a:0".into());
                args.push("-map".into());
                args.push("0:v?".into());
                args.push("-c:v".into());
                args.push("copy".into());
            } else {
                args.push("-vn".into());
            }
            args.push("-map_metadata".into());
            args.push("0".into());
        }
    }
    // The encoder goes here and nowhere else: after everything about the
    // input and the metadata, before the destination. A custom's own
    // arguments take exactly this slot, so what someone types is the
    // encoder half of the line and never the half above it.
    args.extend(format.encoder());
    args.push(item.dest.to_string_lossy().into_owned());
    args
}

/// One selected track as the planner reads it.
pub struct Row {
    pub src: PathBuf,
    /// The span this row is inside its file, None for a plain file.
    pub span: Option<Span>,
    /// The tag values the pattern renders from, and that a span writes.
    pub values: Vec<(Field, String)>,
}

/// Why a selected track produces no file.
#[derive(Clone, PartialEq, Debug)]
pub enum Skip {
    /// Something is already at the destination. Never overwritten.
    Exists,
    /// Another selected track renders the same name.
    Duplicate,
    /// The pattern can't render this row's values.
    Render(String),
}

impl Skip {
    pub fn label(&self) -> String {
        match self {
            Skip::Exists => "already there".into(),
            Skip::Duplicate => "two tracks want this name".into(),
            Skip::Render(e) => e.clone(),
        }
    }
}

/// One planned row: the conversion it would run, and why it won't.
pub struct Entry {
    pub item: Item,
    pub skip: Option<Skip>,
}

impl Entry {
    /// Whether this row actually converts when the run starts.
    pub fn converts(&self) -> bool {
        self.skip.is_none()
    }
}

/// Append `ext` rather than replacing one. `set_extension` eats everything
/// after the last dot of the rendered name, which a title like "R.E.M." or
/// "Vol. 2" leaves plenty of; the rename dialog dodges the same trap.
fn with_extension(path: PathBuf, ext: &str) -> PathBuf {
    let mut name = path.into_os_string();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// Render every row into an output under `dest` and sort out what can
/// actually run. `exists` answers whether a path is taken, injected so the
/// plan can be tested without a filesystem.
pub fn plan(
    rows: &[Row],
    dest: &Path,
    pattern: &guess::Pattern,
    ext: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::with_capacity(rows.len());
    for row in rows {
        let (out, skip) = match pattern.render(&row.values) {
            Ok(rendered) => (with_extension(dest.join(rendered), ext), None),
            // A row that renders nothing still gets an entry, so the
            // preview can say which track it was and why.
            Err(e) => (row.src.clone(), Some(Skip::Render(e))),
        };
        entries.push(Entry {
            item: Item {
                src: row.src.clone(),
                dest: out,
                span: row.span,
                tags: row.values.clone(),
            },
            skip,
        });
    }
    // Two rows onto one name: neither runs. Whichever landed second would
    // either fail on the existing file or, with a different name for the
    // same track, leave nobody able to say which is which.
    let mut wanted: HashMap<PathBuf, usize> = HashMap::new();
    for entry in entries.iter().filter(|e| e.converts()) {
        *wanted.entry(entry.item.dest.clone()).or_default() += 1;
    }
    for entry in entries.iter_mut() {
        if entry.converts() && wanted.get(&entry.item.dest).copied().unwrap_or(0) > 1 {
            entry.skip = Some(Skip::Duplicate);
        }
    }
    for entry in entries.iter_mut() {
        if entry.converts() && exists(&entry.item.dest) {
            entry.skip = Some(Skip::Exists);
        }
    }
    entries
}

/// Live progress of a run: a worker writes it per file, the tasks window
/// polls it.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    failed: AtomicUsize,
    /// Files nothing was written for because something was already at the
    /// destination. Seeded with what the plan skipped before the run began,
    /// and added to by the check a worker makes right before it spawns.
    skipped: AtomicUsize,
    /// Files that came out whole. Lower than `done` after a cancel, where
    /// the file a worker was killed mid-encode counts as gone through
    /// without leaving anything behind.
    wrote: AtomicUsize,
    /// The file a worker is on. Whichever wrote last, so it reads as a
    /// sample of the work rather than a queue position.
    current: Mutex<String>,
    cancel: AtomicBool,
    pace: rox_core::pace::Pace,
}

impl Progress {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn wrote(&self) -> usize {
        self.wrote.load(Ordering::Relaxed)
    }

    pub fn skipped(&self) -> usize {
        self.skipped.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// What a run left behind, for the row that reports on it afterwards.
#[derive(Clone)]
pub struct Summary {
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub dest: PathBuf,
    pub stopped: bool,
}

impl Summary {
    /// The one-line report, the same sentence wherever it's shown.
    pub fn line(&self) -> String {
        let files = if self.converted == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", self.converted)
        };
        let head = if self.stopped {
            format!("Stopped after {files} to {}", self.dest.display())
        } else {
            format!("{files} to {}", self.dest.display())
        };
        let mut line = head;
        if self.skipped > 0 {
            line.push_str(&format!(", {} skipped", self.skipped));
        }
        if self.failed > 0 {
            line.push_str(&format!(", {} failed", self.failed));
        }
        line
    }
}

/// The running conversion, or nothing. App-global so it outlives the dialog
/// that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last run's report, kept for the tasks window until it's dismissed.
#[derive(Default)]
struct Last(Option<Summary>);

impl Global for Last {}

/// The last failure's stderr tail, kept beside the summary: "3 failed" with
/// no reason sends someone to the log, and ffmpeg's last line is usually
/// the whole answer ("Unknown encoder 'libopus'").
#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

/// The running conversion's progress, for a UI that wants to show it.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// How the last run went. None until one has run this session, and None
/// again once its row has been dismissed.
pub fn last(cx: &App) -> Option<Summary> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

/// What ffmpeg said about the last file that failed, if one did.
pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Drop the last run's report, the X on its row.
pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
}

/// Ask the running conversion to stop. Unlike the analysis passes this
/// doesn't wait for the current file: a half-written encode is not a file
/// anyone wants, so the children are killed and their outputs removed.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Convert `items`, writing into whatever folders their destinations name.
/// A no-op while a run is already going: one at a time keeps the machine
/// answerable and the tasks window honest.
pub fn start(items: Vec<Item>, format: Format, dest: PathBuf, skipped: usize, cx: &mut App) {
    if progress(cx).is_some() || items.is_empty() {
        return;
    }
    let binary = binary();
    let progress = Arc::new(Progress::default());
    progress.total.store(items.len(), Ordering::Relaxed);
    progress.skipped.store(skipped, Ordering::Relaxed);
    cx.set_global(Running(Some(progress.clone())));
    // A fresh run's report replaces the last one rather than sitting under
    // it, so the row never shows an old count beside a live bar.
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
    // Nothing observes an app-global job on its own; this is what keeps the
    // tasks window and the menubar chip ticking while it runs.
    crate::tasks_window::repaint_while_running(cx);
    // The run outlives the dialog, which closes on the press, so hand over
    // something that carries the count and the stop button.
    crate::tasks_window::open(cx);
    // Quitting kills the children the same way Stop does. An encode that
    // outlived the app would keep writing into a file nothing is watching.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let failure = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&items, &format, &binary, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            let failed = progress.failed();
            cx.set_global(Last(Some(Summary {
                converted: progress.wrote(),
                skipped: progress.skipped(),
                failed,
                dest,
                stopped: progress.stopping(),
            })));
            if let Some(failure) = failure {
                log::warn!("convert: {failure}");
                cx.set_global(LastFailure(Some(failure)));
            }
        })
        .ok();
    })
    .detach();
}

/// The blocking half: a bounded pool over a cursor, the measuring pass's
/// shape. Returns the first failure's stderr tail, since one reason is what
/// a row can show and they're nearly always the same reason.
fn run(items: &[Item], format: &Format, binary: &str, progress: &Progress) -> Option<String> {
    progress.pace.begin();
    let cursor = AtomicUsize::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
        .min(items.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if !progress.keep_going() {
                    break;
                }
                let Some(item) = items.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                *progress.current.lock().unwrap() = item.src.to_string_lossy().into_owned();
                match convert(item, format, binary, progress) {
                    Ok(Outcome::Wrote) => {
                        progress.wrote.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Outcome::Skipped) => {
                        progress.skipped.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Outcome::Cancelled) => {}
                    Err(e) => {
                        log::warn!("convert: {}: {e}", item.src.display());
                        progress.failed.fetch_add(1, Ordering::Relaxed);
                        let mut failure = failure.lock().unwrap();
                        if failure.is_none() {
                            *failure = Some(e);
                        }
                    }
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    failure.into_inner().unwrap()
}

/// What became of one item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// A file came out.
    Wrote,
    /// Something was already at the destination, so nothing was written and
    /// what was there is untouched.
    Skipped,
    /// The run was cancelled mid-encode; the half-file is gone.
    Cancelled,
}

/// One file through ffmpeg. The output is removed on anything short of a
/// clean exit, cancellation included: what a killed encoder leaves behind
/// is a file that plays for as long as it got, which is worse than no file
/// at all because it looks like one.
fn convert(
    item: &Item,
    format: &Format,
    binary: &str,
    progress: &Progress,
) -> Result<Outcome, String> {
    // Checked here as well as in the plan, because the two happen at
    // different times and a folder can gain a file in between. It also has
    // to be here rather than left to ffmpeg: `-n` does the right thing with
    // an existing file but exits 0 doing it, so a run that trusted the exit
    // status would count every skip as a conversion.
    if item.dest.exists() {
        return Ok(Outcome::Skipped);
    }
    if let Some(dir) = item.dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut child = command(binary)
        .args(args(item, format))
        .spawn()
        .map_err(|e| format!("{binary}: {e}"))?;
    // Drained on its own thread: ffmpeg blocks once the pipe fills, and a
    // file it has a lot to say about would otherwise hang the worker that
    // is meant to be watching it.
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let status = wait(&mut child, progress);
    let stderr = reader.join().unwrap_or_default();
    match status {
        // A clean exit that wrote nothing is the file having appeared under
        // us between the check above and the spawn: `-n` refused it and
        // said so, which is a skip rather than a success.
        Some(status) if status.success() => {
            if item.dest.exists() {
                Ok(Outcome::Wrote)
            } else {
                Ok(Outcome::Skipped)
            }
        }
        Some(_) => {
            let _ = std::fs::remove_file(&item.dest);
            Err(tail(&stderr))
        }
        // Cancelled: the child is already dead, and the half-file goes
        // with it.
        None => {
            let _ = std::fs::remove_file(&item.dest);
            Ok(Outcome::Cancelled)
        }
    }
}

/// Wait for a child, looking up every [`POLL`] to see whether the run was
/// cancelled. None means it was, and the child has been killed.
fn wait(child: &mut Child, progress: &Progress) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            // Treat a child we can't ask about as gone rather than looping
            // on it forever; the missing output is what fails the file.
            Err(_) => return None,
            Ok(None) => {}
        }
        if !progress.keep_going() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// The last of ffmpeg's stderr, which is where the reason is. An empty one
/// still gets a sentence: "it failed" with nothing after it reads as the
/// readout being broken rather than the file.
fn tail(stderr: &str) -> String {
    let text = stderr.trim();
    if text.is_empty() {
        return "ffmpeg failed without saying why".into();
    }
    let start = text.len().saturating_sub(STDERR_TAIL);
    let cut = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= start)
        .unwrap_or(0);
    text[cut..].replace('\n', "; ")
}

/// One of the five as the format the builders take. Test-only sugar, and
/// it sits out here because both tiers below want it.
#[cfg(test)]
fn fixed(preset: Preset) -> Format {
    Format::Preset(preset)
}

/// A custom that parses, for the tests that aren't about why one wouldn't.
#[cfg(test)]
fn custom(ext: &str, args: &str) -> Format {
    Format::Custom(Custom::parse(ext, args).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(values: &[(Field, &str)]) -> Vec<(Field, String)> {
        values
            .iter()
            .map(|(f, v)| (f.clone(), (*v).to_owned()))
            .collect()
    }

    fn plain(dest: &str) -> Item {
        Item {
            src: PathBuf::from("/m/a.flac"),
            dest: PathBuf::from(dest),
            span: None,
            tags: Vec::new(),
        }
    }

    /// A whole file keeps its own metadata and its cover, and the preset's
    /// encoder is the only thing that changes between two of them.
    #[test]
    fn a_plain_file_maps_its_metadata_and_art_through() {
        assert_eq!(
            args(&plain("/out/x.flac"), &fixed(Preset::Flac)),
            [
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "/m/a.flac",
                "-map",
                "0:a:0",
                "-map",
                "0:v?",
                "-c:v",
                "copy",
                "-map_metadata",
                "0",
                "-c:a",
                "flac",
                "-compression_level",
                "8",
                "/out/x.flac",
            ]
        );
        assert_eq!(
            args(&plain("/out/x.mp3"), &fixed(Preset::Mp3V0))[15..],
            ["-c:a", "libmp3lame", "-q:a", "0", "/out/x.mp3"]
        );
    }

    /// A container with nowhere to put a picture drops the video stream
    /// instead of failing the encode over it.
    #[test]
    fn a_container_without_art_takes_the_audio_alone() {
        assert_eq!(
            args(&plain("/out/x.opus"), &fixed(Preset::Opus192)),
            [
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "/m/a.flac",
                "-vn",
                "-map_metadata",
                "0",
                "-c:a",
                "libopus",
                "-b:a",
                "192k",
                "/out/x.opus",
            ]
        );
        assert_eq!(
            args(&plain("/out/x.wav"), &fixed(Preset::Wav))[10..],
            ["-c:a", "pcm_s16le", "/out/x.wav"]
        );
    }

    /// A span trims with input options and writes the row's own tags over
    /// a cleared slate, since the image's describe the whole disc.
    #[test]
    fn a_span_trims_and_writes_the_row_tags() {
        let item = Item {
            src: PathBuf::from("/m/disc.flac"),
            dest: PathBuf::from("/out/04 - Julie.flac"),
            span: Some(Span {
                start_ms: 180_000,
                end_ms: Some(400_500),
            }),
            tags: tags(&[
                (Field::Title, "Julie and Candy"),
                (Field::Artist, "Boards of Canada"),
                (Field::Album, "Geogaddi"),
                (Field::TrackNo, "4"),
                // Not one of the four a span writes, so it stays out of
                // the command line.
                (Field::Genre, "Electronic"),
            ]),
        };
        assert_eq!(
            args(&item, &fixed(Preset::Flac)),
            [
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "180.000",
                "-to",
                "400.500",
                "-i",
                "/m/disc.flac",
                "-vn",
                "-map_metadata",
                "-1",
                "-metadata",
                "title=Julie and Candy",
                "-metadata",
                "artist=Boards of Canada",
                "-metadata",
                "album=Geogaddi",
                "-metadata",
                "track=4",
                "-c:a",
                "flac",
                "-compression_level",
                "8",
                "/out/04 - Julie.flac",
            ]
        );
    }

    /// The last track of a sheet has no end, and runs to the file's.
    #[test]
    fn an_open_ended_span_passes_no_end_mark() {
        let mut item = plain("/out/x.flac");
        item.span = Some(Span {
            start_ms: 2_500,
            end_ms: None,
        });
        let args = args(&item, &fixed(Preset::Flac));
        assert!(args.contains(&"-ss".to_string()));
        assert_eq!(args[6], "2.500");
        assert!(!args.contains(&"-to".to_string()));
    }

    /// Every invocation refuses to overwrite, and none of them ever carries
    /// the flag that would let it. Custom included: a typed argument list
    /// lands in the encoder slot and nowhere near this.
    #[test]
    fn no_invocation_ever_says_yes_to_overwriting() {
        let mut formats: Vec<Format> = Preset::ALL.into_iter().map(fixed).collect();
        formats.push(custom("ogg", "-c:a libvorbis -q:a 6"));
        for format in formats {
            let mut item = plain("/out/x");
            for _ in 0..2 {
                let args = args(&item, &format);
                assert!(args.contains(&"-n".to_string()));
                assert!(!args.contains(&"-y".to_string()));
                item.span = Some(Span {
                    start_ms: 0,
                    end_ms: None,
                });
            }
        }
    }

    /// A custom's arguments are the encoder half of the line and nothing
    /// else: what this module owns still comes first, in the same order it
    /// does for a preset, and the destination still comes last. The cover
    /// stays out, since nothing here knows what an arbitrary container does
    /// with an attached picture.
    #[test]
    fn a_custom_format_slots_its_arguments_where_the_codec_goes() {
        assert_eq!(
            args(
                &plain("/out/x.ogg"),
                &custom("ogg", "-c:a libvorbis -q:a 6")
            ),
            [
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                "/m/a.flac",
                "-vn",
                "-map_metadata",
                "0",
                "-c:a",
                "libvorbis",
                "-q:a",
                "6",
                "/out/x.ogg",
            ]
        );
    }

    /// The span case with a custom on it: the trim and the row's own tags
    /// are still convert.rs's, and the typed arguments still land after
    /// them.
    #[test]
    fn a_custom_span_keeps_the_trim_and_the_row_tags() {
        let item = Item {
            src: PathBuf::from("/m/disc.flac"),
            dest: PathBuf::from("/out/04 - Julie.ogg"),
            span: Some(Span {
                start_ms: 180_000,
                end_ms: Some(400_500),
            }),
            tags: tags(&[
                (Field::Title, "Julie and Candy"),
                (Field::Artist, "Boards of Canada"),
            ]),
        };
        assert_eq!(
            args(&item, &custom("ogg", "-c:a libvorbis -q:a 6")),
            [
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                "180.000",
                "-to",
                "400.500",
                "-i",
                "/m/disc.flac",
                "-vn",
                "-map_metadata",
                "-1",
                "-metadata",
                "title=Julie and Candy",
                "-metadata",
                "artist=Boards of Canada",
                "-c:a",
                "libvorbis",
                "-q:a",
                "6",
                "/out/04 - Julie.ogg",
            ]
        );
    }

    /// The extension is the container, so it's a plain name or it's
    /// nothing.
    #[test]
    fn a_custom_extension_is_a_bare_container_name() {
        assert_eq!(Custom::parse(".OGG", "").unwrap().ext, "ogg");
        assert_eq!(Custom::parse("m4a", "").unwrap().ext, "m4a");
        assert!(Custom::parse("  ", "").is_err());
        assert!(Custom::parse("../etc/passwd", "").is_err());
        assert!(Custom::parse("ogg vorbis", "").is_err());
    }

    /// Everything convert.rs owns is refused by name rather than dropped:
    /// someone who typed `-y` gets told it isn't available, instead of
    /// watching a run behave as though they hadn't.
    #[test]
    fn a_custom_cannot_reach_what_this_module_owns() {
        for line in [
            "-c:a libvorbis -y",
            "-n -c:a libvorbis",
            "-i /etc/passwd",
            "-f matroska",
            "-attach cover.png",
        ] {
            assert!(
                Custom::parse("ogg", line).is_err(),
                "{line} was let through"
            );
        }
        // A bare token where a flag belongs, and a file name sitting in a
        // value slot: both are a second output by any other name.
        assert!(Custom::parse("ogg", "-c:a libvorbis out.ogg").is_err());
        assert!(Custom::parse("ogg", "/tmp/out.ogg").is_err());
        assert!(Custom::parse("ogg", "-c:a /tmp/out.ogg").is_err());
    }

    /// The tokens are whitespace and nothing cleverer, and values that
    /// happen to carry dots or negative numbers survive it.
    #[test]
    fn the_argument_split_is_plain_whitespace() {
        assert_eq!(
            parse_args("  -c:a  libopus\t-b:a 96k ").unwrap(),
            ["-c:a", "libopus", "-b:a", "96k"]
        );
        assert_eq!(parse_args("").unwrap(), Vec::<String>::new());
        assert_eq!(
            parse_args("-af volume=0.5 -ar 44100").unwrap(),
            ["-af", "volume=0.5", "-ar", "44100"]
        );
        assert_eq!(
            parse_args("-map_metadata -1").unwrap(),
            ["-map_metadata", "-1"]
        );
    }

    fn row(src: &str, values: &[(Field, &str)]) -> Row {
        Row {
            src: PathBuf::from(src),
            span: None,
            values: tags(values),
        }
    }

    fn julie(src: &str) -> Row {
        row(
            src,
            &[
                (Field::AlbumArtist, "Boards"),
                (Field::Artist, "Boards"),
                (Field::Album, "Geogaddi"),
                (Field::Title, "Julie"),
                (Field::TrackNo, "4"),
            ],
        )
    }

    fn run_plan(rows: &[Row], pattern: &str, format: Format, taken: &[&str]) -> Vec<Entry> {
        let taken: Vec<PathBuf> = taken.iter().map(PathBuf::from).collect();
        let pattern = guess::parse(pattern).unwrap();
        plan(rows, Path::new("/out"), &pattern, format.ext(), &|path| {
            taken.iter().any(|t| t == path)
        })
    }

    /// The flat default names one file per track under the destination,
    /// with the preset's extension on it rather than the source's.
    #[test]
    fn the_default_pattern_names_files_flat() {
        let got = run_plan(
            &[julie("/m/a.flac")],
            DEFAULT_PATTERN,
            fixed(Preset::Opus192),
            &[],
        );
        assert_eq!(got[0].item.dest, PathBuf::from("/out/Boards - Julie.opus"));
        assert!(got[0].converts());
    }

    /// The mirror toggle is the same render one pattern deeper, folders
    /// and all.
    #[test]
    fn the_mirror_pattern_rebuilds_the_folder_shape() {
        let got = run_plan(
            &[julie("/m/a.flac")],
            MIRROR_PATTERN,
            fixed(Preset::Flac),
            &[],
        );
        assert_eq!(
            got[0].item.dest,
            PathBuf::from("/out/Boards/Geogaddi/04 - Julie.flac")
        );
    }

    /// A destination that exists is left alone. There is no overwrite
    /// anywhere in this feature, so the only thing to decide is whether to
    /// say so, and the row does.
    #[test]
    fn an_existing_destination_is_skipped_never_overwritten() {
        let got = run_plan(
            &[julie("/m/a.flac")],
            DEFAULT_PATTERN,
            fixed(Preset::Flac),
            &["/out/Boards - Julie.flac"],
        );
        assert_eq!(got[0].skip, Some(Skip::Exists));
        assert!(!got[0].converts());
    }

    /// Two tracks that render the same name both stand down, the rename
    /// dialog's rule.
    #[test]
    fn two_tracks_onto_one_name_both_stand_down() {
        let got = run_plan(
            &[
                julie("/m/a.flac"),
                julie("/m/b.flac"),
                row(
                    "/m/c.flac",
                    &[(Field::Artist, "Boards"), (Field::Title, "Candy")],
                ),
            ],
            DEFAULT_PATTERN,
            fixed(Preset::Flac),
            &[],
        );
        assert_eq!(got[0].skip, Some(Skip::Duplicate));
        assert_eq!(got[1].skip, Some(Skip::Duplicate));
        assert!(got[2].converts());
    }

    /// A row the pattern can't render says so in place of its
    /// destination, rather than taking the whole run down.
    #[test]
    fn a_row_that_cannot_render_is_skipped_alone() {
        let got = run_plan(
            &[julie("/m/a.flac"), julie("/m/b.flac")],
            "%skip%/%title%",
            fixed(Preset::Flac),
            &[],
        );
        assert!(matches!(got[0].skip, Some(Skip::Render(_))));
        assert!(matches!(got[1].skip, Some(Skip::Render(_))));
    }

    /// A dotted title keeps its dots and still gets the extension.
    #[test]
    fn the_extension_survives_a_dotted_name() {
        let got = run_plan(
            &[row(
                "/m/a.flac",
                &[(Field::Artist, "R.E.M."), (Field::Title, "Vol. 2")],
            )],
            DEFAULT_PATTERN,
            fixed(Preset::Mp3320),
            &[],
        );
        assert_eq!(got[0].item.dest, PathBuf::from("/out/R.E.M - Vol. 2.mp3"));
    }

    /// A preset survives a round trip through the settings file, and a key
    /// from a build this one doesn't know reads as nothing rather than as
    /// the wrong format.
    #[test]
    fn presets_round_trip_through_their_keys() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_key(preset.key()), Some(preset));
            assert_eq!(fixed(preset).key(), preset.key());
        }
        assert_eq!(Preset::from_key("mp3-v2"), None);
        // The custom key is the one thing in that column that isn't a
        // preset, and no preset can ever claim it.
        assert_eq!(custom("ogg", "").key(), Format::CUSTOM_KEY);
        assert_eq!(Preset::from_key(Format::CUSTOM_KEY), None);
    }

    /// The probe answers false for a binary that isn't there, which is what
    /// hides every surface of the feature.
    #[test]
    fn a_missing_binary_probes_false() {
        assert!(!probe("rox-ffmpeg-that-does-not-exist"));
    }

    /// The failure line carries ffmpeg's own words, on one line, and never
    /// comes back empty.
    #[test]
    fn the_stderr_tail_is_one_readable_line() {
        assert_eq!(
            tail("\nUnknown encoder 'libopus'\n"),
            "Unknown encoder 'libopus'"
        );
        assert_eq!(tail("a\nb"), "a; b");
        assert!(!tail("   ").is_empty());
        let long = "x".repeat(STDERR_TAIL * 2);
        assert_eq!(tail(&long).len(), STDERR_TAIL);
    }
}

/// The tier that actually spawns ffmpeg. Every test here no-ops on a machine
/// without it rather than failing, since the feature no-ops there too: what
/// they're checking is that the command lines above mean what they say when
/// something real reads them, and a build machine with no encoder has
/// nothing to say about that either way.
#[cfg(test)]
mod runtime {
    use super::*;
    use rox_library::writer::Field;

    /// Whether this machine has the binaries, and a line in the test output
    /// when it doesn't, so a skipped tier is never mistaken for a passed one.
    fn ffmpeg_here(what: &str) -> bool {
        if probe("ffmpeg") && probe("ffprobe") {
            return true;
        }
        eprintln!("convert: skipping {what}, no ffmpeg on this machine");
        false
    }

    /// A scratch folder of this test's own, emptied first so a crashed run
    /// leaves nothing behind for the next one to trip on. Never anywhere
    /// near the real library.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-convert-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A tone of `secs` seconds as a FLAC file, the stand-in for a library
    /// file. Mono at 8 kHz keeps it small and quick; nothing here is
    /// listening.
    fn tone(dir: &Path, name: &str, secs: u32) -> PathBuf {
        let path = dir.join(name);
        let status = command("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:duration={secs}"),
                "-ar",
                "8000",
                "-ac",
                "1",
            ])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg would not write the test tone");
        path
    }

    /// What ffprobe says about a file: its duration and its tags, as one
    /// block of text to read assertions out of.
    fn probe_file(path: &Path) -> String {
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:format_tags",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The kinds of stream a file carries, for the art checks.
    fn streams(path: &Path) -> String {
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "csv",
            ])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The duration ffprobe reports, in seconds.
    fn duration(path: &Path) -> f64 {
        probe_file(path)
            .lines()
            .find_map(|line| line.strip_prefix("duration=")?.trim().parse::<f64>().ok())
            .unwrap_or_else(|| panic!("ffprobe said nothing about {}", path.display()))
    }

    /// A whole file through a preset: the audio comes out the other side as
    /// the format asked for, the same length it went in.
    #[test]
    fn a_plain_file_converts_to_opus() {
        if !ffmpeg_here("the plain conversion") {
            return;
        }
        let dir = scratch("plain");
        let src = tone(&dir, "source.flac", 4);
        let item = Item {
            dest: dir.join("out/Tone.opus"),
            src,
            span: None,
            tags: Vec::new(),
        };
        let progress = Progress::default();
        assert_eq!(
            convert(&item, &fixed(Preset::Opus192), "ffmpeg", &progress),
            Ok(Outcome::Wrote)
        );
        assert!(item.dest.is_file());
        // Opus is always 48 kHz and its packets round out, so this is a
        // "the whole thing is there" check rather than a sample count.
        let got = duration(&item.dest);
        assert!((got - 4.0).abs() < 0.2, "{got}s out of a 4s file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The custom path end to end: a typed argument list passes the check
    /// against the real binary, then converts a real file with those same
    /// arguments. The check is the thing worth pinning here, since it's
    /// what the dialog gates Convert on: it has to say yes to something
    /// this ffmpeg can do and no to something it can't.
    #[test]
    fn a_custom_format_checks_and_then_converts() {
        if !ffmpeg_here("the custom format") {
            return;
        }
        let vorbis = Custom::parse("ogg", "-c:a libvorbis -q:a 4").unwrap();
        if let Err(reason) = check(&vorbis) {
            // A build without libvorbis is a different machine, not a
            // regression. The check did its job by saying so.
            eprintln!("convert: skipping the custom format, this ffmpeg said: {reason}");
            return;
        }
        // Nonsense fails, and fails with ffmpeg's own words rather than a
        // shrug, which is what the dialog shows.
        let nonsense = Custom::parse("ogg", "-c:a rox-not-an-encoder").unwrap();
        assert!(check(&nonsense).is_err());
        // The second ask is served from the session cache, so reopening the
        // dialog on a format that already passed spawns nothing.
        assert_eq!(checked(&vorbis), Some(Ok(())));

        let dir = scratch("custom");
        let src = tone(&dir, "source.flac", 3);
        let item = Item {
            dest: dir.join("out/Tone.ogg"),
            src,
            span: None,
            tags: vec![(Field::Title, "Tone".to_owned())],
        };
        let progress = Progress::default();
        assert_eq!(
            convert(&item, &Format::Custom(vorbis), "ffmpeg", &progress),
            Ok(Outcome::Wrote)
        );
        let got = duration(&item.dest);
        assert!((got - 3.0).abs() < 0.2, "{got}s out of a 3s file");
        assert!(
            !streams(&item.dest).contains("video"),
            "a custom carried a picture it was never told the container takes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cover survives into the containers that can hold one, and its
    /// absence doesn't break the ones that can't. The optional video map is
    /// what does both, and it's worth pinning: get it wrong one way and
    /// every conversion loses its art, wrong the other and a source without
    /// art fails to encode at all.
    #[test]
    fn a_cover_rides_along_where_the_container_takes_one() {
        if !ffmpeg_here("the cover art") {
            return;
        }
        let dir = scratch("art");
        let bare = tone(&dir, "bare.flac", 2);
        let with_art = dir.join("art.flac");
        let cover = dir.join("cover.png");
        assert!(command("ffmpeg")
            .args([
                "-nostdin",
                "-n",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=64x64:d=1",
                "-frames:v",
                "1",
            ])
            .arg(&cover)
            .status()
            .unwrap()
            .success());
        assert!(command("ffmpeg")
            .args(["-nostdin", "-n", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(&bare)
            .arg("-i")
            .arg(&cover)
            .args([
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:v",
                "copy",
                "-disposition:v",
                "attached_pic",
                "-c:a",
                "flac",
            ])
            .arg(&with_art)
            .status()
            .unwrap()
            .success());

        let progress = Progress::default();
        let kept = Item {
            src: with_art,
            dest: dir.join("out/kept.flac"),
            span: None,
            tags: Vec::new(),
        };
        assert_eq!(
            convert(&kept, &fixed(Preset::Flac), "ffmpeg", &progress),
            Ok(Outcome::Wrote)
        );
        assert!(
            streams(&kept.dest).contains("video"),
            "the cover didn't come across"
        );
        // The same command line over a file with no picture in it, which is
        // what the "?" on the video map is there for.
        let plain = Item {
            src: bare,
            dest: dir.join("out/plain.mp3"),
            span: None,
            tags: Vec::new(),
        };
        assert_eq!(
            convert(&plain, &fixed(Preset::Mp3320), "ffmpeg", &progress),
            Ok(Outcome::Wrote)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cue case, which is the whole point of the feature: a span of an
    /// image comes out as a standalone file, trimmed to the sheet's window
    /// and wearing the library row's tags rather than the album's.
    #[test]
    fn a_cue_span_converts_to_its_own_trimmed_file() {
        if !ffmpeg_here("the cue span conversion") {
            return;
        }
        let dir = scratch("span");
        let src = tone(&dir, "image.flac", 12);
        let item = Item {
            dest: dir.join("out/04 - Julie.flac"),
            src,
            span: Some(Span {
                start_ms: 3_000,
                end_ms: Some(8_000),
            }),
            tags: vec![
                (Field::Title, "Julie and Candy".to_owned()),
                (Field::Artist, "Boards of Canada".to_owned()),
                (Field::Album, "Geogaddi".to_owned()),
                (Field::TrackNo, "4".to_owned()),
            ],
        };
        let progress = Progress::default();
        assert_eq!(
            convert(&item, &fixed(Preset::Flac), "ffmpeg", &progress),
            Ok(Outcome::Wrote)
        );
        let got = duration(&item.dest);
        assert!((got - 5.0).abs() < 0.1, "{got}s out of a 3s-to-8s window");
        let probed = probe_file(&item.dest).to_lowercase();
        for want in [
            "title=julie and candy",
            "artist=boards of canada",
            "album=geogaddi",
            "track=4",
        ] {
            assert!(probed.contains(want), "{want} missing from {probed}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A destination that exists is never handed to ffmpeg, but if one ever
    /// were, the command line itself refuses it: no -y and no stdin means
    /// the overwrite prompt has nobody to say yes.
    #[test]
    fn an_existing_file_survives_a_conversion_aimed_at_it() {
        if !ffmpeg_here("the overwrite refusal") {
            return;
        }
        let dir = scratch("overwrite");
        let src = tone(&dir, "source.flac", 2);
        let dest = dir.join("taken.flac");
        std::fs::write(&dest, b"not audio").unwrap();
        let item = Item {
            src,
            dest: dest.clone(),
            span: None,
            tags: Vec::new(),
        };
        let progress = Progress::default();
        assert_eq!(
            convert(&item, &fixed(Preset::Flac), "ffmpeg", &progress),
            Ok(Outcome::Skipped)
        );
        // Byte for byte what was there. This is the property the whole
        // feature rests on: a pattern that renders onto an existing file
        // costs that file nothing.
        assert_eq!(std::fs::read(&dest).unwrap(), b"not audio");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancelling kills the encoder and takes the half-file with it. A
    /// partial output is worse than none: it plays for as long as it got,
    /// so it looks like a conversion that worked.
    #[test]
    fn a_cancelled_conversion_leaves_no_partial_file() {
        if !ffmpeg_here("the cancel") {
            return;
        }
        let dir = scratch("cancel");
        // Long enough that the kill lands mid-encode on any machine this
        // runs on, short enough that a machine fast enough to finish it
        // first hasn't wasted anyone's afternoon.
        let src = tone(&dir, "long.flac", 3_600);
        let item = Item {
            dest: dir.join("out/long.flac"),
            src,
            span: None,
            tags: Vec::new(),
        };
        let progress = Arc::new(Progress::default());
        std::thread::spawn({
            let progress = progress.clone();
            move || {
                std::thread::sleep(Duration::from_millis(150));
                progress.cancel.store(true, Ordering::Relaxed);
            }
        });
        match convert(&item, &fixed(Preset::Flac), "ffmpeg", &progress) {
            Ok(Outcome::Cancelled) => assert!(
                !item.dest.exists(),
                "the killed encode left {} behind",
                item.dest.display()
            ),
            // This machine got through an hour of audio in 150ms. Nothing
            // to check, and nothing wrong either.
            Ok(_) => {}
            Err(e) => panic!("the cancelled conversion failed instead: {e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
