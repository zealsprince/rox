//! The metadata writer per ADR 4: tag writes through lofty, wrapped in the
//! copy-verify-rename layer the ADR makes part of this component's
//! definition. lofty rewrites files in place and a failure mid-write can
//! leave one unrecoverable, so the original is never written to: a commit
//! clones the file, writes and verifies the clone, and renames it over the
//! original only once it proves out. A kill at any point leaves either the
//! original or the finished file, never a partial one. Blocking file IO;
//! run it off the UI thread.
//!
//! Fields split two ways, per the component contract. The standard set
//! rides lofty's SplitTag/MergeTag pair, which carries every frame it does
//! not understand (PRIV, GEOB, TXXX, unknown frames) through the write
//! untouched; custom fields go through the format-specific types directly
//! (ID3v2 TXXX, Vorbis keys), because the generic ItemKey has no slot for
//! them.
//!
//! One picture guard rides every commit: an ID3v2.4 tag whose header and
//! APIC frame both flag unsynchronisation reads back mangled through lofty
//! (the art module's carve-out), so a blind read-modify-write would bake
//! that corruption into the file for good. Such a picture is re-read raw
//! and carried through the write, and the verify step compares picture
//! bytes, so committing any field to such a file repairs its tag as a side
//! effect. The raw path recovers one picture, so a multi-picture tag in
//! that shape fails verification instead of writing quietly.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::mpeg::MpegFile;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag, TagItem};

use crate::art;
use crate::embed_tag;
use crate::genre;
use crate::rating;
use crate::replaygain::{self, ReplayGain};

/// A tag field the editor can address. The named set is what the library
/// projects plus the fields a tag editor is expected to carry; `Custom`
/// is a format-specific key, an ID3v2 TXXX description or a Vorbis
/// comment key, written through the format tag so nothing re-maps it.
/// `Rating` speaks the 0-10 display number and fans out to two tag forms
/// on write (whole-star POPM/RATING, exact FMPS_Rating); the rating
/// module owns the conversions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    TrackNo,
    DiscNo,
    Comment,
    Composer,
    /// The unsynchronised lyrics blob (USLT on ID3v2, UNSYNCEDLYRICS on
    /// Vorbis). Free text, newlines and all, including LRC timestamps a
    /// player can sync against; the tag frame never times them itself.
    Lyrics,
    Rating,
    /// One of the four ReplayGain numbers, written by
    /// [`commit_replay_gain`] rather than typed into the editor. It rides
    /// the generic tag like the rest of the named set, which is what makes
    /// it safe: lofty maps the four keys itself (TXXX descriptions on
    /// ID3v2, plain keys on Vorbis, freeform atoms on MP4) and matches
    /// them case-insensitively on the way in, so a set replaces whatever
    /// casing the file already carried instead of landing a second frame
    /// beside it.
    ReplayGain(GainKind),
    Custom(String),
    /// A tag outside the editable set, addressed by the key
    /// [`read_unknown`] lists it under: a TXXX description or bare frame
    /// id on ID3v2, a Vorbis comment key on FLAC, the owner-prefixed
    /// `PRIV:`/`UFID:` forms for the binary carriers. A clear removes
    /// every carrier of the key; a set writes text back through the
    /// key's own carrier - the mapped item where lofty knows the key, a
    /// custom otherwise - so editing a stray tag never lands a TXXX twin
    /// beside the frame it meant to change.
    Unknown(String),
}

/// One slot of a file's ReplayGain. Named for
/// [`crate::replaygain::ReplayGain`]'s own fields so the measured struct
/// and the written tag can't drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainKind {
    TrackDb,
    TrackPeak,
    AlbumDb,
    AlbumPeak,
}

impl GainKind {
    /// The generic key lofty writes this slot through.
    fn item_key(self) -> ItemKey {
        match self {
            GainKind::TrackDb => ItemKey::ReplayGainTrackGain,
            GainKind::TrackPeak => ItemKey::ReplayGainTrackPeak,
            GainKind::AlbumDb => ItemKey::ReplayGainAlbumGain,
            GainKind::AlbumPeak => ItemKey::ReplayGainAlbumPeak,
        }
    }
}

/// One field write; `None` clears the field.
#[derive(Clone, Debug)]
pub struct Change {
    pub field: Field,
    pub value: Option<String>,
}

/// A picture slot the cover editor addresses. The curated set a music
/// library actually carries; lofty's full `PictureType` list is larger,
/// and any type outside this set rides every commit untouched, the same
/// as an unmapped text frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicKind {
    Front,
    Back,
    Media,
    Artist,
}

impl PicKind {
    /// The lofty type a set writes to.
    fn primary_type(self) -> PictureType {
        match self {
            PicKind::Front => PictureType::CoverFront,
            PicKind::Back => PictureType::CoverBack,
            PicKind::Media => PictureType::Media,
            PicKind::Artist => PictureType::Artist,
        }
    }

    /// Every lofty type this slot owns: what a read folds into it and a
    /// write clears before setting. The front slot also owns the untyped
    /// `Other` picture, since a lot of taggers (Windows Media Player among
    /// them) store the album cover there rather than as a typed front, and
    /// an editor that ignored it would show a covered album as empty.
    fn owned_types(self) -> &'static [PictureType] {
        match self {
            PicKind::Front => &[PictureType::CoverFront, PictureType::Other],
            PicKind::Back => &[PictureType::CoverBack],
            PicKind::Media => &[PictureType::Media],
            PicKind::Artist => &[PictureType::Artist],
        }
    }

    /// The slot a lofty picture type maps back to, `None` for the types the
    /// editor leaves alone. Derived from [`Self::owned_types`], so the read
    /// and write agree on which slot a type belongs to.
    fn from_type(kind: PictureType) -> Option<Self> {
        [
            PicKind::Front,
            PicKind::Back,
            PicKind::Media,
            PicKind::Artist,
        ]
        .into_iter()
        .find(|slot| slot.owned_types().contains(&kind))
    }
}

/// One picture write, addressed by slot. `data` `None` removes any
/// picture in that slot; `Some` sets it, replacing an existing picture of
/// the same type. The bytes are the encoded image, the string its mime.
#[derive(Clone, Debug)]
pub struct PicChange {
    pub kind: PicKind,
    pub data: Option<(Vec<u8>, String)>,
}

/// One file's pending edits, the unit `commit_batch` takes: field changes
/// and picture changes, either of which may be empty.
pub struct Edit {
    pub path: PathBuf,
    pub changes: Vec<Change>,
    pub pictures: Vec<PicChange>,
}

/// The named fields' generic keys. `Year` writes the recording date key
/// (TDRC on ID3v2, DATE on Vorbis), the one the scanner's `date()` reads
/// first on both. `Custom` has no generic key by design.
fn item_key(field: &Field) -> Option<ItemKey> {
    Some(match field {
        Field::Title => ItemKey::TrackTitle,
        Field::Artist => ItemKey::TrackArtist,
        Field::Album => ItemKey::AlbumTitle,
        Field::AlbumArtist => ItemKey::AlbumArtist,
        Field::Genre => ItemKey::Genre,
        Field::Year => ItemKey::RecordingDate,
        Field::TrackNo => ItemKey::TrackNumber,
        Field::DiscNo => ItemKey::DiscNumber,
        Field::Comment => ItemKey::Comment,
        Field::Composer => ItemKey::Composer,
        // Always the unsynchronised key on both formats: lofty refuses
        // ItemKey::Lyrics on ID3v2, and UnsyncLyrics carries LRC text
        // through USLT and UNSYNCEDLYRICS the same way.
        Field::Lyrics => ItemKey::UnsyncLyrics,
        Field::ReplayGain(kind) => kind.item_key(),
        // The rating never writes as plain text; `apply_rating` puts its
        // popularimeter form on the generic tag itself.
        Field::Rating | Field::Custom(_) | Field::Unknown(_) => return None,
    })
}

/// The editable field a generic item maps back to, for `read`. `Year`
/// answers for both date keys, mirroring the scanner's fallback.
fn field_of(key: ItemKey) -> Option<Field> {
    Some(match key {
        ItemKey::TrackTitle => Field::Title,
        ItemKey::TrackArtist => Field::Artist,
        ItemKey::AlbumTitle => Field::Album,
        ItemKey::AlbumArtist => Field::AlbumArtist,
        ItemKey::Genre => Field::Genre,
        ItemKey::RecordingDate | ItemKey::Year => Field::Year,
        ItemKey::TrackNumber => Field::TrackNo,
        ItemKey::DiscNumber => Field::DiscNo,
        ItemKey::Comment => Field::Comment,
        ItemKey::Composer => Field::Composer,
        // A file may carry either key (or both, if two apps wrote it);
        // both read back as the one lyrics field, the first wins.
        ItemKey::UnsyncLyrics | ItemKey::Lyrics => Field::Lyrics,
        // ReplayGain is write-only here on purpose: it's a measurement, not
        // something a person types, so it stays out of the editor's field
        // list and out of [`read`]'s named set.
        _ => return None,
    })
}

/// A file's editable fields: the named set in tag order, then the custom
/// fields the format carries (TXXX frames, unmapped Vorbis keys). Fields
/// outside both, sort orders and the like, stay invisible here but ride
/// every commit untouched. Isolated like the scanner's reads: a parser
/// panic costs an error, never the process.
pub fn read(path: &Path) -> Result<Vec<(Field, String)>, String> {
    catch_unwind(AssertUnwindSafe(|| read_inner(path)))
        .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())))
}

fn read_inner(path: &Path) -> Result<Vec<(Field, String)>, String> {
    let kind = file_type(path)?;
    let mut out = Vec::new();
    match kind {
        FileType::Mpeg => {
            let tag = parse_mpeg(path)?.id3v2().cloned().unwrap_or_default();
            named_fields(tag.clone().split_tag().1, &mut out);
            for frame in &tag {
                if let Frame::UserText(f) = frame {
                    if f.description.eq_ignore_ascii_case(rating::FMPS_KEY) {
                        continue;
                    }
                    // An acoustic vector is a few hundred numbers a pass
                    // wrote for the similarity query to read back. Showing it
                    // would put a screenful of base64 in the field list of
                    // every analyzed file, so it stays out of the editor and
                    // out of the metadata panel. Its own module owns it.
                    if embed_tag::is_key(&f.description) {
                        continue;
                    }
                    out.push((
                        Field::Custom(f.description.to_string()),
                        f.content.to_string(),
                    ));
                }
            }
        }
        FileType::Flac => {
            let tag = parse_flac(path)?
                .vorbis_comments()
                .cloned()
                .unwrap_or_default();
            named_fields(tag.clone().split_tag().1, &mut out);
            for (key, value) in tag.items() {
                // Rating-shaped keys stay out of the customs; they show
                // as the one Rating field below instead.
                if key.eq_ignore_ascii_case(rating::FMPS_KEY)
                    || key
                        .get(..7)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("RATING:"))
                {
                    continue;
                }
                // And an acoustic vector, for the reason above: it's a
                // machine's note to itself, not a field anyone edits.
                if embed_tag::is_key(key) {
                    continue;
                }
                if ItemKey::from_key(lofty::tag::TagType::VorbisComments, key).is_none() {
                    out.push((Field::Custom(key.to_string()), value.to_string()));
                }
            }
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
    if let Some(value) = rating::read(path, kind).filter(|v| *v > 0) {
        out.push((Field::Rating, rating::display(value)));
    }
    Ok(out)
}

/// One value in the unknown-tag list: text as the file spells it, or an
/// opaque payload named by its size alone. Binary frames (PRIV, GEOB,
/// UFID) never decode here. The display says how big they are and
/// nothing else, because guessing at their shape would invent structure
/// the tag never promised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownValue {
    Text(String),
    Binary(usize),
}

impl UnknownValue {
    /// The value as one line of display text.
    pub fn display(&self) -> String {
        match self {
            UnknownValue::Text(text) => text.clone(),
            UnknownValue::Binary(bytes) => format!("{} binary", human_bytes(*bytes)),
        }
    }
}

/// A byte count as a short size, decimal units like the file managers
/// show.
fn human_bytes(bytes: usize) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB"] {
        if value < 1000. {
            break;
        }
        value /= 1000.;
        unit = next;
    }
    match unit {
        "B" => format!("{bytes} B"),
        _ => format!("{value:.1} {unit}"),
    }
}

/// Whether a format key stays out of the unknown list. The rating keys
/// have their own field, an acoustic vector is a machine's note to
/// itself, and ReplayGain shows in the library's own column. The
/// writer keeps all three out of the editor, so the read-only list
/// keeps them out too. ReplayGain is named here rather than left to
/// each format because MP3 surfaces the four as TXXX descriptions while
/// FLAC has lofty map them, and one list can't show a gain on one
/// format and hide it on the other.
fn unknown_excluded(key: &str) -> bool {
    if key.eq_ignore_ascii_case(rating::FMPS_KEY) || embed_tag::is_key(key) {
        return true;
    }
    ["RATING:", "REPLAYGAIN_"].iter().any(|prefix| {
        key.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

/// Whether a generic key stays out of the unknown list, the mapped-item
/// side of [`unknown_excluded`]. The popularimeter is where both
/// formats' rating tags land in the generic tag, and the four gains ride
/// item keys of their own.
fn unknown_item_excluded(key: ItemKey) -> bool {
    matches!(
        key,
        ItemKey::Popularimeter
            | ItemKey::ReplayGainTrackGain
            | ItemKey::ReplayGainTrackPeak
            | ItemKey::ReplayGainAlbumGain
            | ItemKey::ReplayGainAlbumPeak
    )
}

/// A file's tags the editor has no row for: the format's custom keys
/// ([`read`]'s customs), the items lofty maps but rox has no field for
/// (BPM, ISRC, the MusicBrainz ids, sort orders), and the ID3v2 frames
/// that carry bytes rather than text. Kept apart from [`read`] on
/// purpose: that one's output feeds the editor's field lookups and the
/// save diff, while this list shows as one ragged set and edits through
/// [`Field::Unknown`] by key. Isolated the same way, so a parser panic
/// costs an error, not the process.
pub fn read_unknown(path: &Path) -> Result<Vec<(String, UnknownValue)>, String> {
    catch_unwind(AssertUnwindSafe(|| read_unknown_inner(path)))
        .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())))
}

fn read_unknown_inner(path: &Path) -> Result<Vec<(String, UnknownValue)>, String> {
    let kind = file_type(path)?;
    let mut out = Vec::new();
    match kind {
        FileType::Mpeg => {
            let (remainder, generic) = parse_mpeg(path)?
                .id3v2()
                .cloned()
                .unwrap_or_default()
                .split_tag();
            mapped_unknowns(&generic, lofty::tag::TagType::Id3v2, None, &mut out);
            // What the split couldn't map: TXXX descriptions, unmapped
            // text frames, and the binary carriers.
            for frame in &*remainder {
                let (key, value) = match frame {
                    Frame::UserText(f) => (
                        f.description.to_string(),
                        UnknownValue::Text(f.content.to_string()),
                    ),
                    Frame::UserUrl(f) => (
                        f.description.to_string(),
                        UnknownValue::Text(f.content.to_string()),
                    ),
                    Frame::Text(f) => (
                        frame.id_str().to_string(),
                        UnknownValue::Text(f.value.to_string()),
                    ),
                    Frame::Url(f) => (
                        frame.id_str().to_string(),
                        UnknownValue::Text(f.url().to_string()),
                    ),
                    Frame::Timestamp(f) => (
                        frame.id_str().to_string(),
                        UnknownValue::Text(f.timestamp.to_string()),
                    ),
                    // The owner names the frame here: a file carries
                    // several PRIVs and they're only told apart by who
                    // wrote them.
                    Frame::Private(f) => (
                        format!("PRIV:{}", f.owner),
                        UnknownValue::Binary(f.private_data.len()),
                    ),
                    Frame::UniqueFileIdentifier(f) => (
                        format!("UFID:{}", f.owner),
                        UnknownValue::Binary(f.identifier.len()),
                    ),
                    Frame::Binary(f) => (
                        frame.id_str().to_string(),
                        UnknownValue::Binary(f.data.len()),
                    ),
                    // Pictures have the cover editor and a bare
                    // popularimeter the rating field; the rest (RVA2,
                    // OWNE, ETCO, TIPL) carry structure a one-line row
                    // would lie about.
                    _ => continue,
                };
                push_unknown(&mut out, key, value);
            }
        }
        FileType::Flac => {
            let tag = parse_flac(path)?
                .vorbis_comments()
                .cloned()
                .unwrap_or_default();
            let vendor = tag.vendor().to_string();
            let (remainder, generic) = tag.split_tag();
            mapped_unknowns(
                &generic,
                lofty::tag::TagType::VorbisComments,
                Some(&vendor),
                &mut out,
            );
            for (key, value) in remainder.items() {
                push_unknown(
                    &mut out,
                    key.to_string(),
                    UnknownValue::Text(value.to_string()),
                );
            }
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
    out.retain(|(key, _)| !unknown_excluded(key));
    Ok(out)
}

/// The generic items lofty mapped that rox has no field for. Labeled by
/// the key the format itself writes them under, so a row reads the same
/// as what another tagger shows for the file; the item key's own name
/// stands in for the rare mapping that has no key on this format.
/// Composer and lyrics are absent by construction: [`field_of`] answers
/// for both, so they're writer-known fields waiting on rows of their
/// own rather than unknowns.
///
/// `vendor` is the FLAC container's vendor string. The Vorbis split
/// injects it as an EncoderSoftware item even when the file carries no
/// such tag, and the encoder's signature is not a tag anyone wrote.
fn mapped_unknowns(
    generic: &Tag,
    tag_type: lofty::tag::TagType,
    vendor: Option<&str>,
    out: &mut Vec<(String, UnknownValue)>,
) {
    for item in generic.items() {
        let key = item.key();
        if field_of(key).is_some() || unknown_item_excluded(key) {
            continue;
        }
        let value = match item.value() {
            ItemValue::Text(text) | ItemValue::Locator(text) => {
                if key == ItemKey::EncoderSoftware && vendor == Some(text.as_str()) {
                    continue;
                }
                UnknownValue::Text(text.clone())
            }
            ItemValue::Binary(bytes) => UnknownValue::Binary(bytes.len()),
        };
        let label = key
            .map_key(tag_type)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{key:?}"));
        push_unknown(out, label, value);
    }
}

/// Add one row, folding a repeated key's text into the "; " list the
/// named fields use for multi-value tags. Two binary frames under one
/// key stay two rows; their sizes are the only thing telling them apart.
fn push_unknown(out: &mut Vec<(String, UnknownValue)>, key: String, value: UnknownValue) {
    if let UnknownValue::Text(text) = &value {
        if let Some((_, UnknownValue::Text(existing))) = out
            .iter_mut()
            .find(|(k, v)| k == &key && matches!(v, UnknownValue::Text(_)))
        {
            existing.push_str("; ");
            existing.push_str(text);
            return;
        }
    }
    out.push((key, value));
}

/// Whether the writer can read and write this file's tags at all. The
/// editor asks before it blames a read failure on the file: an m4a is
/// not a broken file, it's a format the writer hasn't grown a path for.
pub fn supported(path: &Path) -> bool {
    file_type(path).is_ok()
}

/// The named fields out of a split-off generic tag, in item order. Genre
/// is the one multi-value field: its items fold into the single "; "
/// list at the first item's position, so the editor sees the whole list
/// where other readers see only the first value.
fn named_fields(generic: Tag, out: &mut Vec<(Field, String)>) {
    let genres = genre::join(generic.get_strings(ItemKey::Genre));
    let mut genre_taken = false;
    for item in generic.items() {
        let ItemValue::Text(text) = item.value() else {
            continue;
        };
        match field_of(item.key()) {
            Some(Field::Genre) if genre_taken => {}
            Some(Field::Genre) => {
                genre_taken = true;
                out.push((Field::Genre, genres.clone()));
            }
            Some(field) => out.push((field, text.clone())),
            None => {}
        }
    }
}

/// A file's embedded pictures as (type, bytes, mime), read through the
/// source each format actually stores them in.
fn embedded_pictures(
    path: &Path,
    kind: FileType,
) -> Result<Vec<(PictureType, Vec<u8>, String)>, String> {
    Ok(match kind {
        // MP3 keeps its pictures as APIC frames on the ID3v2 tag, which
        // the split moves into the generic picture list.
        FileType::Mpeg => parse_mpeg(path)?
            .id3v2()
            .cloned()
            .unwrap_or_default()
            .split_tag()
            .1
            .pictures()
            .iter()
            .map(pic_tuple)
            .collect(),
        // FLAC keeps its pictures as dedicated PICTURE blocks on the file
        // itself, off the vorbis comments - lofty parses them back there
        // no matter which tag wrote them, so the read and the write both
        // go through the file's own picture store.
        FileType::Flac => parse_flac(path)?
            .pictures()
            .iter()
            .map(|(picture, _)| pic_tuple(picture))
            .collect(),
        _ => unreachable!("file_type only passes writable formats"),
    })
}

/// One picture as (type, bytes, mime), the mime rescued off the magic
/// bytes when the tag declares none or an unknown one, the art module's
/// rule.
fn pic_tuple(picture: &Picture) -> (PictureType, Vec<u8>, String) {
    let mime = match picture.mime_type() {
        Some(MimeType::Unknown(_)) | None => {
            art::sniff(picture.data()).unwrap_or_default().to_string()
        }
        Some(mime) => mime.as_str().to_string(),
    };
    (picture.pic_type(), picture.data().to_vec(), mime)
}

/// A file's embedded pictures at the slots the cover editor addresses,
/// each with its encoded bytes and mime. Exotic-type pictures the editor
/// does not slot are left out here but ride every commit untouched.
/// Isolated like [`read`]: a parser panic costs an error, not the process.
pub fn read_pictures(path: &Path) -> Result<Vec<(PicKind, Vec<u8>, String)>, String> {
    catch_unwind(AssertUnwindSafe(|| read_pictures_inner(path)))
        .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())))
}

fn read_pictures_inner(path: &Path) -> Result<Vec<(PicKind, Vec<u8>, String)>, String> {
    let kind = file_type(path)?;
    let mut out: Vec<(PicKind, Vec<u8>, String)> = embedded_pictures(path, kind)?
        .into_iter()
        .filter_map(|(pic_type, data, mime)| {
            PicKind::from_type(pic_type).map(|slot| (slot, data, mime))
        })
        .collect();
    // The front cover lofty mangles on an unsync MP3 reads clean through
    // the art module's raw path; show that so the diff and the preview see
    // the real image, not the corruption the write itself would repair.
    if kind == FileType::Mpeg {
        if let Some(front) = out.iter_mut().find(|(k, _, _)| *k == PicKind::Front) {
            if let Some((data, mime)) = art::unsync_apic(path, art::ArtKind::Front) {
                front.1 = data;
                front.2 = mime;
            }
        }
    }
    Ok(out)
}

/// Commit changes to one file through the atomic layer: clone, write the
/// clone, verify it (every change reads back, pictures byte-identical,
/// the audio stream hash unchanged), rename it over the original. Any
/// failure, including a parser panic, unlinks the clone and leaves the
/// original byte-identical.
pub fn commit(path: &Path, changes: &[Change]) -> Result<(), String> {
    commit_with(path, changes, &[])
}

/// [`commit`] with picture edits alongside the field changes: the cover
/// editor's path, wrapped in the same atomic layer. Either slice may be
/// empty; a picture-only commit still verifies the fields (a no-op) and
/// the audio hash.
pub fn commit_with(path: &Path, changes: &[Change], pictures: &[PicChange]) -> Result<(), String> {
    let tmp = tmp_path(path);
    let result = catch_unwind(AssertUnwindSafe(|| {
        commit_inner(path, &tmp, changes, pictures)
    }))
    .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Whether a track's tags can go into its file at all. A cue track is a span
/// inside an image every other track of the disc shares, so there is nowhere
/// on disk that means "track 4 of this rip": writing a title would title the
/// whole image, and writing a rating would rate all twelve songs. Only sub 0,
/// a file that is its own track, is writable.
///
/// Editing the sheet itself would be the honest way to change a cue track's
/// tags, and rox does not do that yet. Until it does, these edits live in the
/// library alone.
pub fn writes_to_file(sub: u16) -> bool {
    sub == 0
}

/// [`commit_with`] for a caller holding a subsong key. A cue track's changes
/// never reach the disk: the file write is skipped and Ok comes back, so a
/// rating click or a field edit lands in the library's own row instead of
/// stamping every track of the image. A plain file (sub 0) commits normally.
pub fn commit_key(
    path: &Path,
    sub: u16,
    changes: &[Change],
    pictures: &[PicChange],
) -> Result<(), String> {
    if !writes_to_file(sub) {
        return Ok(());
    }
    commit_with(path, changes, pictures)
}

/// Commit every edit, isolated per file: one malformed file costs its own
/// entry, never the batch. Results come back in edit order.
pub fn commit_batch(edits: &[Edit]) -> Vec<(PathBuf, Result<(), String>)> {
    edits
        .iter()
        .map(|edit| {
            (
                edit.path.clone(),
                commit_with(&edit.path, &edit.changes, &edit.pictures),
            )
        })
        .collect()
}

/// Write a measurement's four ReplayGain numbers into a file's tags, the
/// opt-in half of ADR 19's levelling: the values go to the database by
/// default, and this is what puts them where every other player can read
/// them too. Rides [`commit`], so the whole atomic layer applies - clone,
/// verify, rename - and nothing but the four items moves.
///
/// A `None` field removes that item rather than leaving it alone, so a
/// re-measure that only has a track figure cannot leave last time's album
/// numbers sitting beside it, and an all-`None` call strips a file's
/// ReplayGain outright. Removal goes through the generic key, so it takes
/// the file's item whatever casing the tagger that wrote it used.
///
/// Gains write as `-6.50 dB` and peaks as `0.998762`, the forms
/// [`crate::replaygain`] reads back and every other tagger writes. A
/// non-finite value is dropped rather than written, since `NaN dB` in
/// someone's file is worse than no gain at all.
pub fn commit_replay_gain(path: &Path, gain: ReplayGain) -> Result<(), String> {
    commit(path, &replay_gain_changes(gain))
}

/// Write one model's acoustic vector into a file's tags, the opt-in half of
/// the analysis pass's saving: the vectors go to the database always, and
/// this is the second copy that lets a wiped library or a folder carried to
/// another machine get its descriptions back without decoding everything
/// again.
///
/// Rides [`commit`] through [`Field::Custom`], so the whole atomic layer
/// applies - clone, verify, rename - and the vector lands as an ID3v2 TXXX
/// frame or a Vorbis comment under [`crate::embed_tag`]'s key, which both
/// formats spell the same way. Nothing else in the file moves.
///
/// MP3 and FLAC only, the formats this writer handles at all. Anything else
/// comes back as the error [`file_type`] gives, and the pass treats that as
/// a file that keeps its database row and nothing more.
pub fn commit_embedding(path: &Path, model: &str, vec: &[f32]) -> Result<(), String> {
    commit(
        path,
        &[Change {
            field: Field::Custom(embed_tag::key(model)),
            value: Some(embed_tag::encode(vec)),
        }],
    )
}

/// The same four values as sets alone, with the clears dropped. What
/// [`crate::bake`] writes, and the one place the difference matters.
///
/// A measurement pass writes all four because it just measured all four, so
/// an empty slot there means "this re-measure found no album figure" and
/// clearing is right. An empty slot in a stored row only ever means the
/// database never held that number, and clearing a file's album gain over it
/// would be a tool that claims to add metadata deleting some.
pub fn replay_gain_additions(gain: ReplayGain) -> Vec<Change> {
    replay_gain_changes(gain)
        .into_iter()
        .filter(|change| change.value.is_some())
        .collect()
}

/// The four changes a [`commit_replay_gain`] is: the measured value
/// formatted, or a clear where the measurement has nothing.
fn replay_gain_changes(gain: ReplayGain) -> Vec<Change> {
    let db = |v: Option<f32>| v.filter(|d| d.is_finite()).map(replaygain::format_gain);
    let peak = |v: Option<f32>| v.filter(|p| p.is_finite()).map(format_peak);
    vec![
        Change {
            field: Field::ReplayGain(GainKind::TrackDb),
            value: db(gain.track_db),
        },
        Change {
            field: Field::ReplayGain(GainKind::TrackPeak),
            value: peak(gain.track_peak),
        },
        Change {
            field: Field::ReplayGain(GainKind::AlbumDb),
            value: db(gain.album_db),
        },
        Change {
            field: Field::ReplayGain(GainKind::AlbumPeak),
            value: peak(gain.album_peak),
        },
    ]
}

/// A peak in the form a tag holds it: six decimals of a linear sample
/// value, which is what the RG spec asks for and what every tagger in the
/// wild writes. Plenty of resolution for an f32, and it parses as a plain
/// float everywhere.
fn format_peak(peak: f32) -> String {
    format!("{peak:.6}")
}

fn commit_inner(
    path: &Path,
    tmp: &Path,
    changes: &[Change],
    pictures: &[PicChange],
) -> Result<(), String> {
    let changes = expand_rating(changes);
    let changes = changes.as_slice();
    let kind = file_type(path)?;
    // What must hold after the write: the audio stream untouched and the
    // pictures the edits leave byte-identical, with the raw re-read
    // standing in for the front cover lofty mangles.
    let audio_hash = hash_span(path, audio_span(path, kind)?)?;
    let rescue = if kind == FileType::Mpeg {
        art::unsync_apic(path, art::ArtKind::Front)
    } else {
        None
    };
    // MP3 always verifies its pictures (the unsync hazard); FLAC only when
    // an edit touches them, since lofty otherwise carries its picture
    // blocks through whole.
    let check_pictures = kind == FileType::Mpeg || !pictures.is_empty();
    let expected_pictures = if check_pictures {
        expected_pictures(path, kind, rescue.as_ref(), pictures)?
    } else {
        Vec::new()
    };

    fs::copy(path, tmp).map_err(|e| format!("copy for write: {e}"))?;
    write_tags(tmp, kind, changes, rescue, pictures)?;

    verify_fields(tmp, kind, changes)?;
    if check_pictures {
        verify_pictures(tmp, kind, &expected_pictures)?;
    }
    if hash_span(tmp, audio_span(tmp, kind)?)? != audio_hash {
        return Err("audio stream changed across the write".into());
    }

    // Flush the clone to disk before the rename, or a power cut can leave
    // the original replaced by a truncated file. The handle needs write
    // access: Windows' FlushFileBuffers rejects a read-only one with
    // access denied, so a read-only open fails every save there.
    fs::OpenOptions::new()
        .write(true)
        .open(tmp)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync clone: {e}"))?;
    fs::rename(tmp, path).map_err(|e| format!("rename over original: {e}"))
}

/// Apply the changes to the clone. Customs land on the format tag first;
/// the named set goes through split/merge so every unrecognized frame
/// rides along untouched.
fn write_tags(
    tmp: &Path,
    kind: FileType,
    changes: &[Change],
    rescue: Option<(Vec<u8>, String)>,
    pictures: &[PicChange],
) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tmp)
        .map_err(|e| format!("open for write: {e}"))?;
    match kind {
        FileType::Mpeg => {
            // Zero padding a tagger left outside the declared tag size
            // makes lofty's write probe give up before it finds the audio,
            // so every save fails; fold it into the tag first. Only the
            // clone changes, and only its size field.
            fold_tag_gap(&mut file)?;
            // Read through the sanitiser so a tag lofty would de-unsync
            // twice parses clean; the write below zeroes the header flag,
            // so the saved clone no longer carries the shape at all.
            let mut source = crate::tag_source::open(tmp).map_err(|e| format!("open: {e}"))?;
            let mut mpeg = MpegFile::read_from(&mut source, parse_opts())
                .map_err(|e| format!("parse: {e}"))?;
            let mut tag = mpeg.id3v2().cloned().unwrap_or_default();
            for change in changes {
                match &change.field {
                    Field::Custom(key) => match &change.value {
                        Some(v) => drop(tag.insert_user_text(key.clone(), v.clone())),
                        None => drop(tag.remove_user_text(key)),
                    },
                    Field::Unknown(key) => apply_unknown_mpeg(&mut tag, key, &change.value),
                    _ => {}
                }
            }
            let (remainder, mut generic) = tag.split_tag();
            apply_unknown_generic(&mut generic, lofty::tag::TagType::Id3v2, changes);
            apply_named(&mut generic, changes);
            apply_rating(&mut generic, changes);
            if let Some((data, mime)) = rescue {
                set_front_picture(&mut generic, data, &mime);
            }
            // After the rescue so a front-cover edit overrides the raw
            // re-read of the mangled one rather than the reverse.
            apply_pictures(&mut generic, pictures);
            let mut tag = remainder.merge_tag(generic);
            // lofty writes frame content raw but carries the read tag's
            // header flags along, so a tag read off an unsynchronised
            // file would claim unsynchronisation it no longer has, and
            // the next read would collapse byte pairs that were never
            // stuffed. Nothing lofty writes is unsynchronised; say so.
            let mut flags = *tag.flags();
            flags.unsynchronisation = false;
            tag.set_flags(flags);
            mpeg.set_id3v2(tag);
            file.rewind().map_err(|e| format!("rewind: {e}"))?;
            mpeg.save_to(&mut file, WriteOptions::default())
                .map_err(|e| format!("write: {e}"))
        }
        FileType::Flac => {
            let mut source = crate::tag_source::open(tmp).map_err(|e| format!("open: {e}"))?;
            let mut flac = FlacFile::read_from(&mut source, parse_opts())
                .map_err(|e| format!("parse: {e}"))?;
            let mut tag = flac.vorbis_comments().cloned().unwrap_or_default();
            // Unknowns ride the custom path here: a Vorbis key is its own
            // carrier whichever tier the read filed it under, and a
            // mapped one round-trips through the split unchanged.
            for change in changes {
                match &change.field {
                    Field::Custom(key) | Field::Unknown(key) => {
                        tag.remove(key).for_each(drop);
                        if let Some(v) = &change.value {
                            tag.push(key.clone(), v.clone());
                        }
                    }
                    _ => {}
                }
            }
            let (remainder, mut generic) = tag.split_tag();
            preserve_bare_rating(&mut generic, changes);
            apply_named(&mut generic, changes);
            apply_rating(&mut generic, changes);
            flac.set_vorbis_comments(remainder.merge_tag(generic));
            apply_pictures_flac(&mut flac, pictures);
            file.rewind().map_err(|e| format!("rewind: {e}"))?;
            flac.save_to(&mut file, WriteOptions::default())
                .map_err(|e| format!("write: {e}"))
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
}

/// The named changes onto the generic tag: a set replaces every item of
/// the key, a clear drops them all. A genre set splits its "; " list
/// into one item per value, so the merge writes the format's native
/// multiples (repeated GENRE comments, a null-separated TCON); a list
/// with no values clears like an empty set anywhere else.
fn apply_named(generic: &mut Tag, changes: &[Change]) {
    for change in changes {
        let Some(key) = item_key(&change.field) else {
            continue;
        };
        if change.field == Field::Genre {
            generic.remove_key(key);
            if let Some(v) = &change.value {
                for part in genre::split(v) {
                    generic.push(TagItem::new(
                        ItemKey::Genre,
                        ItemValue::Text(part.to_string()),
                    ));
                }
            }
            continue;
        }
        match &change.value {
            Some(v) => drop(generic.insert_text(key, v.clone())),
            None => generic.remove_key(key),
        }
    }
}

/// The key [`read_unknown`] files an ID3v2 frame under, the address an
/// unknown edit removes by. Mirrors the read's naming: descriptions for
/// the user frames, the owner-prefixed forms for PRIV and UFID, the
/// frame id for everything else.
fn mpeg_unknown_key(frame: &Frame<'_>) -> String {
    match frame {
        Frame::UserText(f) => f.description.to_string(),
        Frame::UserUrl(f) => f.description.to_string(),
        Frame::Private(f) => format!("PRIV:{}", f.owner),
        Frame::UniqueFileIdentifier(f) => format!("UFID:{}", f.owner),
        _ => frame.id_str().to_string(),
    }
}

/// One unknown change onto an MP3's format tag, ahead of the split:
/// every frame the key names goes, whatever tier carried it. A set whose
/// key lofty has no mapping for lands back as a TXXX here; a mapped one
/// waits for [`apply_unknown_generic`], so the value writes through the
/// format's own frame instead.
fn apply_unknown_mpeg(tag: &mut Id3v2Tag, key: &str, value: &Option<String>) {
    tag.retain(|frame| mpeg_unknown_key(frame) != key);
    if let Some(v) = value {
        if ItemKey::from_key(lofty::tag::TagType::Id3v2, key).is_none() {
            drop(tag.insert_user_text(key.to_string(), v.clone()));
        }
    }
}

/// The mapped half of an unknown set, after the split: a key lofty knows
/// writes through its generic item and merges back into the frame the
/// file carried it in. Clears need nothing here - the format pass
/// already dropped every carrier.
fn apply_unknown_generic(generic: &mut Tag, tag_type: lofty::tag::TagType, changes: &[Change]) {
    for change in changes {
        let Field::Unknown(key) = &change.field else {
            continue;
        };
        let Some(v) = &change.value else { continue };
        if let Some(item_key) = ItemKey::from_key(tag_type, key) {
            generic.insert_text(item_key, v.clone());
        }
    }
}

/// A rating change fanned out ahead of the write and the verify: the
/// value normalized to its canonical display form (zero clears), plus
/// its exact FMPS custom, which rides the ordinary custom path in both
/// formats. The whole-star half goes through [`apply_rating`].
fn expand_rating(changes: &[Change]) -> Vec<Change> {
    let mut out = Vec::with_capacity(changes.len() + 1);
    for change in changes {
        if change.field != Field::Rating {
            out.push(change.clone());
            continue;
        }
        let value = change
            .value
            .as_deref()
            .and_then(rating::parse_display)
            .filter(|v| *v > 0);
        out.push(Change {
            field: Field::Rating,
            value: value.map(rating::display),
        });
        out.push(Change {
            field: Field::Custom(rating::FMPS_KEY.into()),
            value: value.map(rating::fmps),
        });
    }
    out
}

/// The rating changes onto the generic tag: the whole-star popularimeter
/// with an empty email, which lofty merges to a bare POPM frame on ID3v2
/// and a bare RATING key on Vorbis - the forms other players read. One
/// rating per file: a set replaces every popularimeter, whoever wrote it.
fn apply_rating(generic: &mut Tag, changes: &[Change]) {
    for change in changes {
        if change.field != Field::Rating {
            continue;
        }
        match change.value.as_deref().and_then(rating::parse_display) {
            Some(v) if v > 0 => {
                generic.insert_text(ItemKey::Popularimeter, rating::popm_text(v));
            }
            _ => generic.remove_key(ItemKey::Popularimeter),
        }
    }
}

/// lofty's Vorbis split hands a bare RATING key through as its raw
/// number, but its merge only writes the email|stars|counter form back,
/// so any commit would silently drop a rating another app left there.
/// Reformat it - at whole-star resolution, all the form carries - when
/// this commit brings no rating of its own.
fn preserve_bare_rating(generic: &mut Tag, changes: &[Change]) {
    if changes.iter().any(|c| c.field == Field::Rating) {
        return;
    }
    let Some(raw) = generic
        .get_string(ItemKey::Popularimeter)
        .map(str::to_string)
    else {
        return;
    };
    if raw.contains('|') {
        return;
    }
    if let Some(value) = rating::parse_popm_text(&raw).filter(|v| *v > 0) {
        generic.insert_text(ItemKey::Popularimeter, rating::popm_text(value));
    }
}

/// Swap the rescued raw picture bytes in for the front cover lofty read
/// mangled, or the first picture failing that, keeping its declared type.
/// The description does not survive the swap; the image does.
fn set_front_picture(generic: &mut Tag, data: Vec<u8>, mime: &str) {
    let ix = generic
        .pictures()
        .iter()
        .position(|p| p.pic_type() == PictureType::CoverFront)
        .unwrap_or(0);
    let pic_type = generic
        .pictures()
        .get(ix)
        .map_or(PictureType::CoverFront, Picture::pic_type);
    let picture = Picture::unchecked(data)
        .pic_type(pic_type)
        .mime_type(MimeType::from_str(mime))
        .build();
    if generic.pictures().is_empty() {
        generic.push_picture(picture);
    } else {
        generic.set_picture(ix, picture);
    }
}

/// The picture edits onto the generic tag, addressed by slot type: a set
/// replaces the picture of that type or pushes a new one, a remove drops
/// every picture of that type. [`expected_pictures`] mirrors this exactly,
/// so the verify step compares the write against the same transformation.
fn apply_pictures(generic: &mut Tag, pictures: &[PicChange]) {
    for change in pictures {
        // Drop every type the slot owns first, so a set leaves one and a
        // remove leaves none; [`expected_pictures`] does the same.
        for &pic_type in change.kind.owned_types() {
            generic.remove_picture_type(pic_type);
        }
        if let Some((data, mime)) = &change.data {
            let picture = Picture::unchecked(data.clone())
                .pic_type(change.kind.primary_type())
                .mime_type(MimeType::from_str(mime))
                .build();
            generic.push_picture(picture);
        }
    }
}

/// The picture edits onto a FLAC file, through its own picture store: a
/// set drops the slot's type and inserts the new picture, a remove drops
/// it. Kept apart from [`apply_pictures`] because lofty holds FLAC
/// pictures off the vorbis comments the generic tag round-trips.
fn apply_pictures_flac(flac: &mut FlacFile, pictures: &[PicChange]) {
    for change in pictures {
        for &pic_type in change.kind.owned_types() {
            flac.remove_picture_type(pic_type);
        }
        if let Some((data, mime)) = &change.data {
            let picture = Picture::unchecked(data.clone())
                .pic_type(change.kind.primary_type())
                .mime_type(MimeType::from_str(mime))
                .build();
            // The information block is a read-time convenience; real
            // players size off the image itself, so a picture that will
            // not parse still writes with a zeroed block rather than
            // failing the commit.
            let info = PictureInformation::from_picture(&picture).unwrap_or_default();
            let _ = flac.insert_picture(picture, Some(info));
        }
    }
}

/// Every change read back off the clone, checked against what was asked.
/// Customs read through the format tag, the named set through a fresh
/// split, so the check exercises the same path the next scan will.
fn verify_fields(tmp: &Path, kind: FileType, changes: &[Change]) -> Result<(), String> {
    let custom_keys = changes.iter().filter_map(|c| match &c.field {
        Field::Custom(key) => Some(key.clone()),
        _ => None,
    });
    let (generic, customs): (Tag, Vec<(String, Option<String>)>) = match kind {
        FileType::Mpeg => {
            let tag = parse_mpeg(tmp)?.id3v2().cloned().unwrap_or_default();
            let customs = custom_keys
                .map(|key| {
                    let value = tag.get_user_text(&key).map(str::to_string);
                    (key, value)
                })
                .collect();
            (tag.split_tag().1, customs)
        }
        FileType::Flac => {
            let tag = parse_flac(tmp)?
                .vorbis_comments()
                .cloned()
                .unwrap_or_default();
            let customs = custom_keys
                .map(|key| {
                    let value = tag.get(&key).map(str::to_string);
                    (key, value)
                })
                .collect();
            (tag.split_tag().1, customs)
        }
        _ => unreachable!("file_type only passes writable formats"),
    };
    // Unknown changes verify through the same list the editor showed
    // them in, so the check exercises what the next open will read.
    let unknowns: Vec<(String, UnknownValue)> =
        if changes.iter().any(|c| matches!(c.field, Field::Unknown(_))) {
            read_unknown_inner(tmp)?
        } else {
            Vec::new()
        };
    for change in changes {
        // The rating verifies at star resolution: its popularimeter is
        // the whole-star form by design, and a FLAC hands it back as the
        // bare number rather than the written text. The exact value
        // verifies through its FMPS custom like any other.
        if change.field == Field::Rating {
            let expected = change
                .value
                .as_deref()
                .and_then(rating::parse_display)
                .map(rating::stars);
            let got = generic
                .get_string(ItemKey::Popularimeter)
                .and_then(rating::parse_popm_text)
                .filter(|v| *v > 0)
                .map(rating::stars);
            if got != expected {
                return Err(format!(
                    "verify: rating read back {got:?} stars, expected {expected:?}"
                ));
            }
            continue;
        }
        // Genre verifies at the "; " list level on both sides: the write
        // splits the value into items, so the read-back rejoins them, and
        // the asked-for value canonicalizes so "Rock;;Pop " proves out as
        // "Rock; Pop". A list with no values wrote nothing, like a clear.
        if change.field == Field::Genre {
            let expected = change
                .value
                .as_deref()
                .map(genre::canonical)
                .filter(|v| !v.is_empty());
            let read_back =
                Some(genre::join(generic.get_strings(ItemKey::Genre))).filter(|v| !v.is_empty());
            if read_back != expected {
                return Err(format!(
                    "verify: {:?} read back {:?}, expected {:?}",
                    change.field, read_back, expected
                ));
            }
            continue;
        }
        let read_back = match &change.field {
            Field::Custom(key) => customs
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.clone()),
            Field::Unknown(key) => unknowns
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.display()),
            named => generic
                .get_string(item_key(named).expect("named fields have keys"))
                .map(str::to_string),
        };
        if read_back != change.value {
            return Err(format!(
                "verify: {:?} read back {:?}, expected {:?}",
                change.field, read_back, change.value
            ));
        }
    }
    Ok(())
}

/// The pictures the clone must carry: what lofty reads off the original,
/// the rescued raw bytes standing in for the front cover it mangles, then
/// the picture edits applied. The rescue substitution and the edit
/// application mirror [`set_front_picture`] and [`apply_pictures`] step
/// for step (both formats through their own picture store), so a clean
/// write reads back exactly this multiset.
fn expected_pictures(
    path: &Path,
    kind: FileType,
    rescue: Option<&(Vec<u8>, String)>,
    pictures: &[PicChange],
) -> Result<Vec<Vec<u8>>, String> {
    let mut items: Vec<(PictureType, Vec<u8>)> = embedded_pictures(path, kind)?
        .into_iter()
        .map(|(pic_type, data, _)| (pic_type, data))
        .collect();
    // The rescue swaps the front cover (or the first picture failing that),
    // keeping the slot's type; an empty tag gains a front.
    if let Some((data, _)) = rescue {
        let ix = items
            .iter()
            .position(|(t, _)| *t == PictureType::CoverFront)
            .unwrap_or(0);
        match items.get_mut(ix) {
            Some(slot) => slot.1 = data.clone(),
            None => items.push((PictureType::CoverFront, data.clone())),
        }
    }
    for change in pictures {
        for &pic_type in change.kind.owned_types() {
            items.retain(|(t, _)| *t != pic_type);
        }
        if let Some((data, _)) = &change.data {
            items.push((change.kind.primary_type(), data.clone()));
        }
    }
    Ok(items.into_iter().map(|(_, data)| data).collect())
}

/// The clone's pictures against the expected set, compared as byte
/// multisets: the write may reorder frames, it may only touch an image an
/// edit named.
fn verify_pictures(tmp: &Path, kind: FileType, expected: &[Vec<u8>]) -> Result<(), String> {
    let mut got: Vec<Vec<u8>> = embedded_pictures(tmp, kind)?
        .into_iter()
        .map(|(_, data, _)| data)
        .collect();
    let mut want = expected.to_vec();
    got.sort();
    want.sort();
    if got != want {
        return Err(format!(
            "pictures changed across the write: {} in, {} out",
            want.len(),
            got.len()
        ));
    }
    Ok(())
}

/// The formats the writer handles today, off the file's content. The rest
/// of the library's matrix (wav) fails per file here until it gets its own
/// write path.
fn file_type(path: &Path) -> Result<FileType, String> {
    let kind = Probe::open(path)
        .map_err(|e| format!("open: {e}"))?
        .guess_file_type()
        .map_err(|e| format!("probe: {e}"))?
        .file_type()
        .ok_or_else(|| format!("unrecognized format: {}", path.display()))?;
    match kind {
        FileType::Mpeg | FileType::Flac => Ok(kind),
        other => Err(format!("writing {other:?} tags is not supported yet")),
    }
}

/// Fold the junk sitting between the declared ID3v2 tag end and the
/// first MPEG sync back into the tag, by growing the header's size field
/// over it. The junk is a tagger's leavings, padding written outside the
/// tag size or the headless carcass of a frame the tag was written over,
/// and it breaks every write, because lofty re-detects the format
/// mid-save and gives up after 1024 junk bytes. Growing the size makes
/// the save's rewrite of the tag region swallow the junk for good. Runs
/// on the writer's clone only, and only when a sync actually follows;
/// anything else is left to fail as it would today.
fn fold_tag_gap(file: &mut fs::File) -> Result<(), String> {
    let Some(gap) = crate::tag_source::tag_gap(file).map_err(|e| format!("gap scan: {e}"))? else {
        return Ok(());
    };
    if gap.junk == 0 || !gap.sync {
        return Ok(());
    }
    let grown = u64::from(gap.size) + gap.junk;
    if grown >= 1 << 28 {
        return Ok(()); // past the synchsafe ceiling; leave the file alone
    }
    file.seek(SeekFrom::Start(6))
        .and_then(|_| file.write_all(&art::synchsafe_encode(grown as u32)))
        .map_err(|e| format!("fold junk: {e}"))
}

/// Whether the writer's parser reads `path` clean, for the repair scan:
/// `Err` carries the parse error a repair pass should surface. A format
/// outside the writer's matrix reads as fine, since a rewrite could do
/// nothing for it anyway.
pub fn readable(path: &Path) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(kind) = file_type(path) else {
            return Ok(());
        };
        match kind {
            FileType::Mpeg => parse_mpeg(path).map(drop),
            FileType::Flac => parse_flac(path).map(drop),
            _ => Ok(()),
        }
    }))
    .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())))
}

/// Tags only; the writer never needs the stream properties, and skipping
/// them lets a file with a garbled stream still get its tags fixed.
fn parse_opts() -> ParseOptions {
    crate::parse_opts().read_properties(false)
}

fn parse_mpeg(path: &Path) -> Result<MpegFile, String> {
    let mut source = crate::tag_source::open(path).map_err(|e| format!("open: {e}"))?;
    MpegFile::read_from(&mut source, parse_opts()).map_err(|e| format!("parse: {e}"))
}

fn parse_flac(path: &Path) -> Result<FlacFile, String> {
    let mut source = crate::tag_source::open(path).map_err(|e| format!("open: {e}"))?;
    FlacFile::read_from(&mut source, parse_opts()).map_err(|e| format!("parse: {e}"))
}

/// The suffix the writer's working clone carries beside the original while
/// a commit runs. Public so the library watcher can tell the writer's own
/// clone-and-rename traffic from real changes.
pub const CLONE_SUFFIX: &str = ".rox-write";

/// Whether a path is the writer's working clone.
pub fn is_clone_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(CLONE_SUFFIX))
}

/// The clone's path: a sibling in the same directory, so the final rename
/// never crosses a filesystem, with an extension the scanner ignores.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(CLONE_SUFFIX);
    path.with_file_name(name)
}

/// The byte range holding the audio stream, so its hash can prove the
/// write only moved tags. MP3: past the leading ID3v2 tag (footer
/// included), short of trailing ID3v1 and APE tags. FLAC: past the
/// metadata blocks, which is where every tag lives.
fn audio_span(path: &Path, kind: FileType) -> Result<(u64, u64), String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let len = file.metadata().map_err(|e| format!("stat: {e}"))?.len();
    match kind {
        FileType::Mpeg => {
            let mut start = 0u64;
            let mut header = [0u8; 10];
            if file.read_exact(&mut header).is_ok() && &header[..3] == b"ID3" {
                let size = art::synchsafe(&header[6..10]).ok_or("malformed ID3v2 size")? as u64;
                let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
                start = 10 + size + footer;
            }
            // Junk between the declared tag end and the first sync (a
            // tagger's out-of-tag padding, a frame carcass the tag was
            // written over) is not audio, and a repair write drops it:
            // hash from the sync, the same boundary the fold uses, so
            // the span agrees before and after. A file with no sync at
            // all keeps its declared start; no write survives on it
            // anyway.
            file.seek(SeekFrom::Start(start.min(len)))
                .map_err(|e| format!("seek: {e}"))?;
            let (junk, sync) =
                crate::tag_source::scan_to_sync(&mut file, crate::tag_source::GAP_SCAN_CAP)
                    .map_err(|e| format!("read: {e}"))?;
            if sync {
                start += junk;
            }
            let mut end = len;
            if end >= start + 128 {
                let mut magic = [0u8; 3];
                file.seek(SeekFrom::Start(end - 128))
                    .and_then(|_| file.read_exact(&mut magic))
                    .map_err(|e| format!("read: {e}"))?;
                if &magic == b"TAG" {
                    end -= 128;
                }
            }
            if end >= start + 32 {
                let mut footer = [0u8; 32];
                file.seek(SeekFrom::Start(end - 32))
                    .and_then(|_| file.read_exact(&mut footer))
                    .map_err(|e| format!("read: {e}"))?;
                if &footer[..8] == b"APETAGEX" {
                    // The footer's size counts the items and itself; the
                    // header, when the flags claim one, sits on top.
                    let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
                    let flags = u32::from_le_bytes(footer[20..24].try_into().unwrap());
                    let header = if flags & (1 << 31) != 0 { 32 } else { 0 };
                    end = end.saturating_sub(size + header);
                }
            }
            Ok((start.min(len), end.max(start.min(len))))
        }
        FileType::Flac => {
            let mut magic = [0u8; 4];
            file.read_exact(&mut magic)
                .map_err(|e| format!("read: {e}"))?;
            if &magic != b"fLaC" {
                return Err("not a flac stream".into());
            }
            let mut pos = 4u64;
            loop {
                let mut block = [0u8; 4];
                file.seek(SeekFrom::Start(pos))
                    .and_then(|_| file.read_exact(&mut block))
                    .map_err(|e| format!("read: {e}"))?;
                let size = u32::from_be_bytes([0, block[1], block[2], block[3]]) as u64;
                pos += 4 + size;
                if block[0] & 0x80 != 0 {
                    break;
                }
            }
            Ok((pos.min(len), len))
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
}

/// FNV-1a over the span, chunked. The stream is a few megabytes and the
/// hash guards against a moved boundary, not an adversary.
fn hash_span(path: &Path, (start, end): (u64, u64)) -> Result<u64, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek: {e}"))?;
    let mut remaining = end - start;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = [0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let got = file
            .read(&mut buf[..want])
            .map_err(|e| format!("read: {e}"))?;
        if got == 0 {
            break;
        }
        for &b in &buf[..got] {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        remaining -= got as u64;
    }
    Ok(hash)
}

/// The tag fixtures, for the modules that write through this one and want
/// a real file under their tests rather than a second copy of the bytes.
#[cfg(test)]
pub(crate) use tests::{flac_file, mp3_file, scratch};

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-writer-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set(field: Field, value: &str) -> Change {
        Change {
            field,
            value: Some(value.to_string()),
        }
    }

    fn clear(field: Field) -> Change {
        Change { field, value: None }
    }

    /// Three contiguous MPEG1 Layer3 frames (128 kbps, 44.1 kHz, 417
    /// bytes each) with patterned payloads: enough structure that lofty's
    /// property reader accepts the stream, enough entropy that a moved or
    /// truncated span cannot hash the same.
    fn mpeg_audio() -> Vec<u8> {
        let mut audio = Vec::new();
        for frame in 0..3u32 {
            audio.extend([0xFF, 0xFB, 0x90, 0x00]);
            audio.extend((0..413u32).map(|i| ((frame * 413 + i) * 7 % 251) as u8));
        }
        audio
    }

    pub(crate) fn mp3_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, mpeg_audio()).unwrap();
        path
    }

    /// A bare FLAC container: magic, one last-flagged STREAMINFO claiming
    /// 44.1 kHz stereo 16-bit, then patterned bytes standing in for the
    /// frames.
    pub(crate) fn flac_file(dir: &Path, name: &str) -> PathBuf {
        let mut bytes = b"fLaC".to_vec();
        bytes.extend([0x80, 0, 0, 34]);
        let mut info = [0u8; 34];
        info[..4].copy_from_slice(&[0x10, 0x00, 0x10, 0x00]);
        info[10..18].copy_from_slice(&[0x0A, 0xC4, 0x42, 0xF0, 0, 0, 0, 0]);
        bytes.extend(info);
        bytes.extend((0..600u32).map(|i| (i * 11 % 253) as u8));
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn value_of(fields: &[(Field, String)], field: &Field) -> Option<String> {
        fields
            .iter()
            .find(|(f, _)| f == field)
            .map(|(_, v)| v.clone())
    }

    fn unknown_of(rows: &[(String, UnknownValue)], key: &str) -> Option<UnknownValue> {
        rows.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn text_of(rows: &[(String, UnknownValue)], key: &str) -> Option<String> {
        match unknown_of(rows, key) {
            Some(UnknownValue::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// The unknown list's three tiers on one MP3: the TXXX descriptions
    /// [`read`] already surfaces, the frames lofty maps to item keys rox
    /// has no field for, and the binary carriers named by size alone.
    /// The excluded families ride the same file, so a leak in any of
    /// them fails here.
    #[test]
    fn mp3_unknown_tags_cover_the_three_tiers() {
        use lofty::id3::v2::{
            BinaryFrame, ExtendedTextFrame, FrameId, Id3v2Tag, PrivateFrame, TextInformationFrame,
        };
        use lofty::TextEncoding;
        use std::borrow::Cow;

        let dir = scratch("mp3-unknown");
        let path = mp3_file(&dir, "track.mp3");
        let txxx = |description: &str, content: &str| {
            Frame::UserText(ExtendedTextFrame::new(
                TextEncoding::UTF8,
                description.to_string(),
                content.to_string(),
            ))
        };
        let mut tag = Id3v2Tag::default();
        tag.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TIT2")),
            TextEncoding::UTF8,
            "Known",
        )));
        // Tier a: descriptions nothing maps.
        tag.insert(txxx("MY NOTE", "kept"));
        // Tier b: mapped, but the editor has no row for either.
        tag.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TBPM")),
            TextEncoding::UTF8,
            "128",
        )));
        tag.insert(txxx("MusicBrainz Artist Id", "f4ab-1"));
        // The exclusions.
        tag.insert(txxx("REPLAYGAIN_TRACK_GAIN", "-7.35 dB"));
        tag.insert(txxx(rating::FMPS_KEY, "0.8"));
        tag.insert(txxx(&embed_tag::key("test-model"), "v1;dim=2;f16;AAAA"));
        // Tier c: bytes, never decoded.
        tag.insert(Frame::Private(PrivateFrame::new(
            "rox.test",
            vec![7u8; 1500],
        )));
        tag.insert(Frame::Binary(BinaryFrame::new(
            FrameId::Valid(Cow::Borrowed("GEOB")),
            vec![3u8; 2048],
        )));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "MY NOTE").as_deref(), Some("kept"));
        assert_eq!(text_of(&rows, "TBPM").as_deref(), Some("128"));
        assert_eq!(
            text_of(&rows, "MusicBrainz Artist Id").as_deref(),
            Some("f4ab-1")
        );
        assert_eq!(
            unknown_of(&rows, "PRIV:rox.test"),
            Some(UnknownValue::Binary(1500))
        );
        assert_eq!(unknown_of(&rows, "GEOB"), Some(UnknownValue::Binary(2048)));
        assert_eq!(
            unknown_of(&rows, "PRIV:rox.test").unwrap().display(),
            "1.5 KB binary"
        );
        // The title has a row of its own, and the three excluded
        // families have no business here at all.
        for key in [
            "TIT2",
            "REPLAYGAIN_TRACK_GAIN",
            rating::FMPS_KEY,
            &embed_tag::key("test-model"),
        ] {
            assert!(
                unknown_of(&rows, key).is_none(),
                "{key} must stay out of the unknown list"
            );
        }
    }

    /// The FLAC side of the same list, and the format asymmetry it
    /// closes: lofty maps the ReplayGain keys here and leaves them as
    /// TXXX descriptions on MP3, so only an explicit exclusion keeps
    /// both formats showing the same thing.
    #[test]
    fn flac_unknown_tags_cover_the_tiers_and_exclusions() {
        use lofty::ogg::VorbisComments;

        let dir = scratch("flac-unknown");
        let path = flac_file(&dir, "track.flac");
        let mut tag = VorbisComments::default();
        tag.push("TITLE".into(), "Known".into());
        // Tier a, then tier b: unmapped key, then two lofty maps.
        tag.push("MY NOTE".into(), "kept".into());
        tag.push("BPM".into(), "128".into());
        tag.push("MUSICBRAINZ_ARTISTID".into(), "f4ab-1".into());
        // The exclusions, rating in both of its Vorbis shapes.
        tag.push("REPLAYGAIN_TRACK_GAIN".into(), "-7.35 dB".into());
        tag.push(rating::FMPS_KEY.into(), "0.8".into());
        tag.push("RATING:rox@example.com".into(), "196".into());
        tag.push(embed_tag::key("test-model"), "v1;dim=2;f16;AAAA".into());
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "MY NOTE").as_deref(), Some("kept"));
        assert_eq!(text_of(&rows, "BPM").as_deref(), Some("128"));
        assert_eq!(
            text_of(&rows, "MUSICBRAINZ_ARTISTID").as_deref(),
            Some("f4ab-1")
        );
        for key in [
            "TITLE",
            "REPLAYGAIN_TRACK_GAIN",
            rating::FMPS_KEY,
            "RATING:rox@example.com",
            &embed_tag::key("test-model"),
            // The split hands the container's vendor string over as an
            // encoder tag the file never carried.
            "ENCODER",
        ] {
            assert!(
                unknown_of(&rows, key).is_none(),
                "{key} must stay out of the unknown list"
            );
        }
    }

    /// An unknown edit lands through the key's own carrier and a clear
    /// removes every one: the TXXX tier, the mapped tier (where a set
    /// must not leave a TXXX twin beside the real frame), and the
    /// binary tier, which only clears.
    #[test]
    fn mp3_unknown_edits_address_every_tier() {
        use lofty::id3::v2::{
            ExtendedTextFrame, FrameId, Id3v2Tag, PrivateFrame, TextInformationFrame,
        };
        use lofty::TextEncoding;
        use std::borrow::Cow;

        let dir = scratch("mp3-unknown-edit");
        let path = mp3_file(&dir, "track.mp3");
        let mut tag = Id3v2Tag::default();
        tag.insert(Frame::UserText(ExtendedTextFrame::new(
            TextEncoding::UTF8,
            "MY NOTE".to_string(),
            "old".to_string(),
        )));
        tag.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TBPM")),
            TextEncoding::UTF8,
            "128",
        )));
        tag.insert(Frame::Private(PrivateFrame::new("rox.test", vec![7u8; 64])));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        commit(
            &path,
            &[
                set(Field::Unknown("MY NOTE".into()), "new"),
                set(Field::Unknown("TBPM".into()), "90"),
            ],
        )
        .unwrap();
        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "MY NOTE").as_deref(), Some("new"));
        assert_eq!(text_of(&rows, "TBPM").as_deref(), Some("90"));
        let tag = parse_mpeg(&path).unwrap().id3v2().cloned().unwrap();
        assert!(
            tag.get_user_text("TBPM").is_none(),
            "a mapped set must not leave a TXXX twin"
        );

        commit(
            &path,
            &[
                clear(Field::Unknown("MY NOTE".into())),
                clear(Field::Unknown("TBPM".into())),
                clear(Field::Unknown("PRIV:rox.test".into())),
            ],
        )
        .unwrap();
        let rows = read_unknown(&path).unwrap();
        for key in ["MY NOTE", "TBPM", "PRIV:rox.test"] {
            assert!(unknown_of(&rows, key).is_none(), "{key} should be gone");
        }
    }

    /// The FLAC side of the same edits: one flat key space, so the
    /// unmapped and mapped tiers write and clear alike.
    #[test]
    fn flac_unknown_edits_write_and_clear_by_key() {
        use lofty::ogg::VorbisComments;

        let dir = scratch("flac-unknown-edit");
        let path = flac_file(&dir, "track.flac");
        let mut tag = VorbisComments::default();
        tag.push("MY NOTE".into(), "old".into());
        tag.push("BPM".into(), "128".into());
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        commit(
            &path,
            &[
                set(Field::Unknown("MY NOTE".into()), "new"),
                set(Field::Unknown("BPM".into()), "90"),
            ],
        )
        .unwrap();
        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "MY NOTE").as_deref(), Some("new"));
        assert_eq!(text_of(&rows, "BPM").as_deref(), Some("90"));

        commit(
            &path,
            &[
                clear(Field::Unknown("MY NOTE".into())),
                clear(Field::Unknown("BPM".into())),
            ],
        )
        .unwrap();
        let rows = read_unknown(&path).unwrap();
        for key in ["MY NOTE", "BPM"] {
            assert!(unknown_of(&rows, key).is_none(), "{key} should be gone");
        }
    }

    /// A format the writer has no path for answers plainly rather than
    /// looking like a broken file.
    #[test]
    fn unknown_tags_refuse_an_unsupported_format() {
        let dir = scratch("unknown-unsupported");
        let path = dir.join("track.wav");
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(36u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend([1, 0, 1, 0]);
        bytes.extend(44100u32.to_le_bytes());
        bytes.extend(88200u32.to_le_bytes());
        bytes.extend([2, 0, 16, 0]);
        bytes.extend(b"data");
        bytes.extend(0u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();

        assert!(!supported(&path));
        assert!(read_unknown(&path).is_err());
    }

    #[test]
    fn mp3_fields_round_trip_over_untouched_audio() {
        let dir = scratch("mp3-round-trip");
        let path = mp3_file(&dir, "track.mp3");
        commit(
            &path,
            &[
                set(Field::Title, "Ninety"),
                set(Field::Artist, "Nine"),
                set(Field::Year, "2020"),
                set(Field::Custom("ROX_TEST".into()), "kept"),
            ],
        )
        .unwrap();
        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Title).as_deref(), Some("Ninety"));
        assert_eq!(value_of(&fields, &Field::Artist).as_deref(), Some("Nine"));
        assert_eq!(value_of(&fields, &Field::Year).as_deref(), Some("2020"));
        assert_eq!(
            value_of(&fields, &Field::Custom("ROX_TEST".into())).as_deref(),
            Some("kept")
        );
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.ends_with(&mpeg_audio()), "audio must survive whole");
    }

    #[test]
    fn flac_fields_round_trip_over_untouched_audio() {
        let dir = scratch("flac-round-trip");
        let path = flac_file(&dir, "track.flac");
        commit(
            &path,
            &[
                set(Field::Title, "Stream"),
                set(Field::AlbumArtist, "Info"),
                set(Field::Custom("ROX_TEST".into()), "kept"),
            ],
        )
        .unwrap();
        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Title).as_deref(), Some("Stream"));
        assert_eq!(
            value_of(&fields, &Field::AlbumArtist).as_deref(),
            Some("Info")
        );
        assert_eq!(
            value_of(&fields, &Field::Custom("ROX_TEST".into())).as_deref(),
            Some("kept")
        );
        let audio: Vec<u8> = (0..600u32).map(|i| (i * 11 % 253) as u8).collect();
        assert!(fs::read(&path).unwrap().ends_with(&audio));
    }

    /// Editing any field leaves a file's ReplayGain where it was. lofty
    /// maps these to item keys rather than carrying them as unknown
    /// frames, so they ride the split/merge with the named fields; a save
    /// that dropped them would silently unlevel a track and there'd be
    /// nothing in the library to notice it with.
    #[test]
    fn replaygain_survives_a_field_edit() {
        let dir = scratch("replaygain-kept");

        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            commit(
                &path,
                &[
                    set(Field::Custom("REPLAYGAIN_TRACK_GAIN".into()), "-7.35 dB"),
                    set(Field::Custom("REPLAYGAIN_TRACK_PEAK".into()), "0.987654"),
                ],
            )
            .unwrap();
            // A later edit to something else entirely, the ordinary case.
            commit(&path, &[set(Field::Title, "Levelled")]).unwrap();

            let rg = crate::scanner::read_one(&path).unwrap().replay_gain;
            assert_eq!(rg.track_db, Some(-7.35), "{}", path.display());
            assert_eq!(rg.track_peak, Some(0.987654), "{}", path.display());
        }
    }

    /// The whole point of writing a vector into a file: the database can be
    /// thrown away and the description comes back off the files, without a
    /// second afternoon of decoding.
    ///
    /// Runs the real path both ways on both writable formats - the pass's
    /// write, then the pick-up a pass does before it decodes anything - with
    /// the row deleted in between, which is what a wiped library or a folder
    /// carried to another machine looks like from here.
    #[test]
    fn a_vector_written_into_a_file_outlives_its_database_row() {
        use crate::embeddings;

        let dir = scratch("embedding-round-trip");
        // Wide enough that the value is a real base64 blob rather than a few
        // characters, and spread across the scales the raw features live on.
        let vec: Vec<f32> = (0..64)
            .map(|i| (i as f32 - 32.0) * 0.37 + (i as f32) * (i as f32) * 0.02)
            .collect();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::init_schema(&conn).unwrap();

        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            let name = path.display().to_string();
            commit_embedding(&path, "builtin-v1", &vec).unwrap();

            // The tag editor and the metadata panel never see it. Without
            // the read skip every analyzed file grows a row of base64 here.
            let fields = read(&path).unwrap();
            assert!(
                !fields
                    .iter()
                    .any(|(f, _)| matches!(f, Field::Custom(k) if embed_tag::is_key(k))),
                "the vector must stay out of the field list, {name}"
            );

            // Analyzed once, into a database that then goes away.
            conn.execute(
                "INSERT INTO tracks (path, title, artist, album, genre, year, track_no,
                    duration_ms, size, mtime)
                 VALUES (?1, 'T', 'A', 'Al', 'g', 0, 1, 200000, 0, 0)",
                rusqlite::params![name],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            embeddings::upsert(&conn, id, "builtin-v1", &vec).unwrap();
            embeddings::clear(&conn, "builtin-v1").unwrap();
            assert_eq!(embeddings::vector(&conn, id, "builtin-v1").unwrap(), None);

            // The pick-up: what the pass tries before it opens a decoder.
            let recovered = embed_tag::read(&path, "builtin-v1", vec.len())
                .unwrap_or_else(|| panic!("no vector came back off {name}"));
            embeddings::upsert(&conn, id, "builtin-v1", &recovered).unwrap();
            let stored = embeddings::vector(&conn, id, "builtin-v1")
                .unwrap()
                .unwrap();
            assert_eq!(stored.len(), vec.len(), "{name}");
            for (a, b) in vec.iter().zip(&stored) {
                let tolerance = (a.abs() * 1e-3).max(1e-6);
                assert!((a - b).abs() <= tolerance, "{a} came back as {b}, {name}");
            }

            // Another model's key is a different key, so a file described by
            // two models hands each one only its own.
            assert!(embed_tag::read(&path, "panns-cnn10", vec.len()).is_none());
            // And a model whose width changed under the same name is refused
            // rather than half-read.
            assert!(embed_tag::read(&path, "builtin-v1", vec.len() + 1).is_none());
        }
    }

    /// The generic tag a read splits out of either format: the same view
    /// the scanner hands the ReplayGain parser.
    fn generic_tag(path: &Path) -> Tag {
        match file_type(path).unwrap() {
            FileType::Mpeg => {
                parse_mpeg(path)
                    .unwrap()
                    .id3v2()
                    .cloned()
                    .unwrap_or_default()
                    .split_tag()
                    .1
            }
            FileType::Flac => {
                parse_flac(path)
                    .unwrap()
                    .vorbis_comments()
                    .cloned()
                    .unwrap_or_default()
                    .split_tag()
                    .1
            }
            _ => unreachable!("the fixtures are mp3 and flac"),
        }
    }

    /// A measurement written back to the file: the four numbers land in
    /// the standard string forms, read back through the ReplayGain parser
    /// as the numbers that went in, and the fields the commit never named
    /// come through untouched. Then a second write with only a track gain
    /// clears the other three, the re-measure case.
    #[test]
    fn replay_gain_writes_the_four_tags_and_clears_on_none() {
        let dir = scratch("replay-gain-write");
        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            let file = path.display().to_string();
            commit(
                &path,
                &[
                    set(Field::Title, "Measured"),
                    set(Field::Custom("MOOD_ROX".into()), "calm"),
                ],
            )
            .unwrap();

            commit_replay_gain(
                &path,
                ReplayGain {
                    track_db: Some(-6.5),
                    track_peak: Some(0.998762),
                    album_db: Some(-8.1),
                    album_peak: Some(1.023),
                },
            )
            .unwrap();

            // The strings another player reads, not just what lofty hands
            // back through its own round trip.
            let tag = generic_tag(&path);
            assert_eq!(
                tag.get_string(ItemKey::ReplayGainTrackGain),
                Some("-6.50 dB"),
                "{file}"
            );
            assert_eq!(
                tag.get_string(ItemKey::ReplayGainTrackPeak),
                Some("0.998762"),
                "{file}"
            );
            assert_eq!(
                tag.get_string(ItemKey::ReplayGainAlbumGain),
                Some("-8.10 dB"),
                "{file}"
            );
            assert_eq!(
                tag.get_string(ItemKey::ReplayGainAlbumPeak),
                Some("1.023000"),
                "{file}"
            );

            // And back through the parser the scanner reads with.
            let rg = replaygain::read(&tag);
            assert_eq!(rg.track_db, Some(-6.5), "{file}");
            assert_eq!(rg.track_peak, Some(0.998762), "{file}");
            assert_eq!(rg.album_db, Some(-8.1), "{file}");
            assert_eq!(rg.album_peak, Some(1.023), "{file}");

            // Nothing else moved.
            let fields = read(&path).unwrap();
            assert_eq!(
                value_of(&fields, &Field::Title).as_deref(),
                Some("Measured"),
                "{file}"
            );
            assert_eq!(
                value_of(&fields, &Field::Custom("MOOD_ROX".into())).as_deref(),
                Some("calm"),
                "{file}"
            );

            // A re-measure with only a track figure takes the rest away.
            commit_replay_gain(
                &path,
                ReplayGain {
                    track_db: Some(-6.0),
                    ..ReplayGain::default()
                },
            )
            .unwrap();
            let rg = replaygain::read(&generic_tag(&path));
            assert_eq!(rg.track_db, Some(-6.0), "{file}");
            assert_eq!(rg.track_peak, None, "{file}");
            assert_eq!(rg.album_db, None, "{file}");
            assert_eq!(rg.album_peak, None, "{file}");

            // Clearing all four leaves a file with no ReplayGain at all.
            commit_replay_gain(&path, ReplayGain::default()).unwrap();
            assert_eq!(
                replaygain::read(&generic_tag(&path)),
                ReplayGain::default(),
                "{file}"
            );
            assert_eq!(
                value_of(&read(&path).unwrap(), &Field::Title).as_deref(),
                Some("Measured"),
                "{file}"
            );
        }
    }

    /// The casing carve-out: plenty of taggers write the ID3v2 TXXX
    /// descriptions lowercase, and a write that matched them literally
    /// would clear nothing and land a second frame beside the stale one.
    /// Going through the generic key means lofty's case-insensitive
    /// mapping does the matching, so a set replaces and a clear removes.
    #[test]
    fn replay_gain_replaces_a_differently_cased_tag() {
        let mut body = vec![0x00]; // latin-1
        body.extend(b"replaygain_track_gain\0");
        body.extend(b"-3.00 dB");
        let mut frames = b"TXXX".to_vec();
        frames.extend(synch(body.len() as u32));
        frames.extend([0x00, 0x00]);
        frames.extend(&body);
        let mut bytes = b"ID3\x04\x00\x00".to_vec();
        bytes.extend(synch(frames.len() as u32));
        bytes.extend(&frames);
        bytes.extend(mpeg_audio());

        let dir = scratch("replay-gain-case");
        let path = dir.join("track.mp3");
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            replaygain::read(&generic_tag(&path)).track_db,
            Some(-3.0),
            "the fixture starts out levelled"
        );

        commit_replay_gain(
            &path,
            ReplayGain {
                track_db: Some(-9.25),
                ..ReplayGain::default()
            },
        )
        .unwrap();
        assert_eq!(replaygain::read(&generic_tag(&path)).track_db, Some(-9.25));
        // One frame, not the new one shadowing an untouched old one.
        let raw = parse_mpeg(&path).unwrap().id3v2().cloned().unwrap();
        let descriptions: Vec<String> = (&raw)
            .into_iter()
            .filter_map(|frame| match frame {
                Frame::UserText(f) => Some(f.description.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(descriptions, ["REPLAYGAIN_TRACK_GAIN"]);

        commit_replay_gain(&path, ReplayGain::default()).unwrap();
        assert_eq!(replaygain::read(&generic_tag(&path)), ReplayGain::default());
        assert!(fs::read(&path).unwrap().ends_with(&mpeg_audio()));
    }

    /// A "; " genre list writes as each format's native multiples - two
    /// GENRE comments on FLAC, one null-separated TCON on ID3v2 - and
    /// reads back rejoined. The typed value canonicalizes on the way
    /// through, and an empty list clears the field.
    #[test]
    fn genre_list_round_trips_as_native_multiples() {
        let dir = scratch("genre-multi");

        let mp3 = mp3_file(&dir, "track.mp3");
        commit(&mp3, &[set(Field::Genre, "Electronic; Ambient")]).unwrap();
        let fields = read(&mp3).unwrap();
        assert_eq!(
            value_of(&fields, &Field::Genre).as_deref(),
            Some("Electronic; Ambient")
        );
        let generic = parse_mpeg(&mp3)
            .unwrap()
            .id3v2()
            .unwrap()
            .clone()
            .split_tag()
            .1;
        let parts: Vec<&str> = generic.get_strings(ItemKey::Genre).collect();
        assert_eq!(parts, ["Electronic", "Ambient"]);

        let flac = flac_file(&dir, "track.flac");
        commit(&flac, &[set(Field::Genre, " Electronic ;; Ambient")]).unwrap();
        let fields = read(&flac).unwrap();
        assert_eq!(
            value_of(&fields, &Field::Genre).as_deref(),
            Some("Electronic; Ambient")
        );
        let vorbis = parse_flac(&flac)
            .unwrap()
            .vorbis_comments()
            .unwrap()
            .clone();
        let parts: Vec<&str> = vorbis.get_all("GENRE").collect();
        assert_eq!(parts, ["Electronic", "Ambient"]);

        // A single value stays a single item, and clearing drops them all.
        commit(&flac, &[set(Field::Genre, "Jazz")]).unwrap();
        let vorbis = parse_flac(&flac)
            .unwrap()
            .vorbis_comments()
            .unwrap()
            .clone();
        assert_eq!(vorbis.get_all("GENRE").count(), 1);
        commit(&flac, &[set(Field::Genre, " ; ")]).unwrap();
        assert_eq!(value_of(&read(&flac).unwrap(), &Field::Genre), None);
    }

    /// The retention half of the contract: a commit naming one field must
    /// carry every other field through untouched, customs included.
    #[test]
    fn unrelated_commit_keeps_other_fields() {
        let dir = scratch("retention");
        let path = mp3_file(&dir, "track.mp3");
        commit(
            &path,
            &[
                set(Field::Title, "Original"),
                set(Field::Custom("MOOD_ROX".into()), "calm"),
            ],
        )
        .unwrap();
        commit(&path, &[set(Field::Artist, "Someone")]).unwrap();
        let fields = read(&path).unwrap();
        assert_eq!(
            value_of(&fields, &Field::Title).as_deref(),
            Some("Original")
        );
        assert_eq!(
            value_of(&fields, &Field::Custom("MOOD_ROX".into())).as_deref(),
            Some("calm")
        );
    }

    /// The rating's fan-out and round trip on both formats: the exact
    /// half-point value survives through FMPS, the whole-star companion
    /// lands beside it, clearing removes both, and the FMPS custom never
    /// shows up as a custom field.
    #[test]
    fn rating_round_trips_with_half_points() {
        let dir = scratch("rating");
        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            commit(&path, &[set(Field::Rating, "7.5")]).unwrap();
            let fields = read(&path).unwrap();
            assert_eq!(value_of(&fields, &Field::Rating).as_deref(), Some("7.5"));
            assert!(
                !fields.iter().any(
                    |(f, _)| matches!(f, Field::Custom(k) if k.eq_ignore_ascii_case("FMPS_Rating"))
                ),
                "the FMPS carrier reads as the rating, not a custom"
            );
            assert_eq!(crate::rating::read_path(&path), Some(75));

            commit(&path, &[clear(Field::Rating)]).unwrap();
            assert_eq!(value_of(&read(&path).unwrap(), &Field::Rating), None);
            assert_eq!(crate::rating::read_path(&path), None);
        }
    }

    /// The lofty 0.24 carve-out this module papers over: a bare Vorbis
    /// RATING key survives an unrelated commit (at star resolution)
    /// instead of being dropped by the asymmetric split/merge pair.
    #[test]
    fn unrelated_flac_commit_keeps_a_bare_rating() {
        let dir = scratch("bare-rating");
        let path = flac_file(&dir, "track.flac");
        commit(&path, &[set(Field::Custom("RATING".into()), "80")]).unwrap();
        commit(&path, &[set(Field::Title, "Untouched rating")]).unwrap();
        assert_eq!(
            value_of(&read(&path).unwrap(), &Field::Rating).as_deref(),
            Some("8")
        );
    }

    #[test]
    fn clearing_removes_the_field() {
        let dir = scratch("clear");
        let path = mp3_file(&dir, "track.mp3");
        commit(&path, &[set(Field::Comment, "temporary")]).unwrap();
        commit(&path, &[clear(Field::Comment)]).unwrap();
        assert_eq!(value_of(&read(&path).unwrap(), &Field::Comment), None);
    }

    /// The atomic layer's observable face: a file the writer cannot
    /// handle comes through a failed commit byte-identical, with no clone
    /// left behind.
    #[test]
    fn failure_leaves_the_original_and_no_clone() {
        let dir = scratch("failure");
        let path = dir.join("bad.mp3");
        fs::write(&path, b"nothing resembling an audio stream").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(commit(&path, &[set(Field::Title, "Nope")]).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!tmp_path(&path).exists(), "the clone must be unlinked");
    }

    #[test]
    fn batch_isolates_the_malformed_file() {
        let dir = scratch("batch");
        let good = mp3_file(&dir, "good.mp3");
        let bad = dir.join("bad.mp3");
        fs::write(&bad, b"nothing resembling an audio stream").unwrap();
        let edits = vec![
            Edit {
                path: good.clone(),
                changes: vec![set(Field::Title, "Made it")],
                pictures: Vec::new(),
            },
            Edit {
                path: bad,
                changes: vec![set(Field::Title, "Nope")],
                pictures: Vec::new(),
            },
        ];
        let results = commit_batch(&edits);
        assert!(results[0].1.is_ok());
        assert!(results[1].1.is_err());
        assert_eq!(
            value_of(&read(&good).unwrap(), &Field::Title).as_deref(),
            Some("Made it")
        );
    }

    /// The unsynchronisation an encoder applies: a zero stuffed after
    /// every `ff` that precedes a zero or a sync-shaped byte. The same
    /// recipe as the art module's test, because this is the same shape.
    fn stuff(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, b) in data.iter().enumerate() {
            out.push(*b);
            if *b == 0xFF && data.get(i + 1).is_some_and(|n| *n == 0x00 || *n >= 0xE0) {
                out.push(0x00);
            }
        }
        out
    }

    fn synch(n: u32) -> [u8; 4] {
        [
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]
    }

    /// The malformed date shape that used to cost the whole file: a TDRC
    /// lofty cannot parse as a timestamp ("06-08", no year) fails the
    /// read outright at the default parsing mode. Relaxed parsing drops
    /// that one frame; everything else stays readable and writable.
    #[test]
    fn malformed_date_frame_costs_only_itself() {
        let mut frames = Vec::new();
        for (id, text) in [(b"TIT2", "Harry"), (b"TDRC", "06-08")] {
            frames.extend(id);
            frames.extend(synch(text.len() as u32 + 1));
            frames.extend([0x00, 0x00]);
            frames.push(0x00); // latin-1
            frames.extend(text.as_bytes());
        }
        let mut bytes = b"ID3\x04\x00\x00".to_vec();
        bytes.extend(synch(frames.len() as u32));
        bytes.extend(&frames);
        bytes.extend(mpeg_audio());

        let dir = scratch("bad-date");
        let path = dir.join("track.mp3");
        fs::write(&path, bytes).unwrap();

        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Title).as_deref(), Some("Harry"));
        assert_eq!(value_of(&fields, &Field::Year), None);

        commit(&path, &[set(Field::Artist, "Highland")]).unwrap();
        let fields = read(&path).unwrap();
        assert_eq!(
            value_of(&fields, &Field::Artist).as_deref(),
            Some("Highland")
        );
        assert_eq!(value_of(&fields, &Field::Title).as_deref(), Some("Harry"));
    }

    /// The acceptance bullet this module carries for the Bandcamp shape:
    /// an ID3v2.4 tag whose header and APIC frame both flag
    /// unsynchronisation reads back mangled through lofty, so a text
    /// commit that trusted the read would corrupt the cover for good. The
    /// rescue path must hand the picture through byte-identical.
    #[test]
    fn text_commit_keeps_unsync_apic_bytes() {
        let image = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0x00, 0x59, 0xFF, 0xFF, 0xD9,
        ];
        let mut body = vec![0x00];
        body.extend(b"image/jpeg\0");
        body.push(3); // front cover
        body.extend(b"c\0");
        body.extend(image);
        let stored = stuff(&body);
        let mut frame = b"APIC".to_vec();
        frame.extend(synch(stored.len() as u32 + 4));
        frame.extend([0x00, 0x03]); // unsynchronised, data length indicator
        frame.extend(synch(body.len() as u32));
        frame.extend(&stored);
        let mut tag = b"ID3\x04\x00\x80".to_vec();
        tag.extend(synch(frame.len() as u32));
        tag.extend(&frame);

        let dir = scratch("unsync-apic");
        let path = dir.join("track.mp3");
        let mut bytes = tag;
        bytes.extend(mpeg_audio());
        fs::write(&path, bytes).unwrap();

        commit(&path, &[set(Field::Title, "Fixed")]).unwrap();
        let (cover, mime) = crate::art::cover_art(&path).expect("the cover must survive");
        assert_eq!(cover, image);
        assert_eq!(mime, "image/jpeg");
        assert_eq!(
            value_of(&read(&path).unwrap(), &Field::Title).as_deref(),
            Some("Fixed")
        );
    }

    /// The repair path the tag repair window drives: a file in the
    /// double-unsync shape flags for repair, a no-op commit rewrites it
    /// clean through the atomic layer, and the same file no longer flags -
    /// with its cover carried through byte-identical.
    #[test]
    fn no_op_commit_repairs_the_unsync_shape() {
        let image = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0x00, 0x59, 0xFF, 0xFF, 0xD9,
        ];
        let mut body = vec![0x00];
        body.extend(b"image/jpeg\0");
        body.push(3); // front cover
        body.extend(b"c\0");
        body.extend(image);
        let stored = stuff(&body);
        let mut frame = b"APIC".to_vec();
        frame.extend(synch(stored.len() as u32 + 4));
        frame.extend([0x00, 0x03]); // unsynchronised, data length indicator
        frame.extend(synch(body.len() as u32));
        frame.extend(&stored);
        let mut tag = b"ID3\x04\x00\x80".to_vec();
        tag.extend(synch(frame.len() as u32));
        tag.extend(&frame);

        let dir = scratch("no-op-repair");
        let path = dir.join("track.mp3");
        let mut bytes = tag;
        bytes.extend(mpeg_audio());
        fs::write(&path, bytes).unwrap();

        assert!(
            crate::tag_source::needs_repair(&path),
            "the shape flags before repair"
        );
        commit(&path, &[]).unwrap();
        assert!(
            !crate::tag_source::needs_repair(&path),
            "the rewrite clears the shape"
        );
        let (cover, mime) = crate::art::cover_art(&path).expect("the cover survives the repair");
        assert_eq!(cover, image);
        assert_eq!(mime, "image/jpeg");
        assert!(fs::read(&path).unwrap().ends_with(&mpeg_audio()));
    }

    /// The out-of-tag padding shape: a tagger left zeros between the
    /// declared tag end and the first MPEG frame, deeper than lofty's
    /// write probe searches, so every save died before writing a byte.
    /// The commit folds the padding into the tag and lands the edit,
    /// with the audio carried through untouched.
    #[test]
    fn commit_folds_padding_left_outside_the_tag() {
        let mut frames = b"TIT2".to_vec();
        frames.extend(synch("Warchief".len() as u32 + 1));
        frames.extend([0x00, 0x00]);
        frames.push(0x00); // latin-1
        frames.extend(b"Warchief");
        let mut bytes = b"ID3\x04\x00\x00".to_vec();
        bytes.extend(synch(frames.len() as u32));
        bytes.extend(&frames);
        bytes.extend(std::iter::repeat_n(0u8, 1500)); // past the 1024-byte probe limit
        bytes.extend(mpeg_audio());

        let dir = scratch("fold-gap");
        let path = dir.join("track.mp3");
        fs::write(&path, bytes).unwrap();

        assert!(crate::tag_source::needs_repair(&path), "the gap flags");
        commit(&path, &[set(Field::Artist, "Redpill")]).unwrap();
        assert!(
            !crate::tag_source::needs_repair(&path),
            "the fold clears it"
        );
        let fields = read(&path).unwrap();
        assert_eq!(
            value_of(&fields, &Field::Title).as_deref(),
            Some("Warchief")
        );
        assert_eq!(
            value_of(&fields, &Field::Artist).as_deref(),
            Some("Redpill")
        );
        assert!(fs::read(&path).unwrap().ends_with(&mpeg_audio()));
    }

    /// The stray-null shape: one surplus byte on a UTF-16 text frame
    /// blanked the whole tag through lofty. A no-op commit reads through
    /// the sanitiser's trim and rewrites the tag clean.
    #[test]
    fn no_op_commit_repairs_the_stray_utf16_null() {
        let title = "Everybody's Safe Until\u{2026}";
        let mut body = vec![0x01]; // utf16 encoding byte
        body.extend([0xFF, 0xFE]); // little-endian BOM
        for ch in title.encode_utf16() {
            body.extend(ch.to_le_bytes());
        }
        body.extend([0x00, 0x00]); // terminator
        body.push(0x00); // the stray byte
        let mut frames = b"TIT2".to_vec();
        frames.extend((body.len() as u32).to_be_bytes()); // v2.3: a plain word
        frames.extend([0x00, 0x00]);
        frames.extend(&body);
        let mut bytes = b"ID3\x03\x00\x00".to_vec();
        bytes.extend(synch(frames.len() as u32));
        bytes.extend(&frames);
        bytes.extend(mpeg_audio());

        let dir = scratch("stray-null");
        let path = dir.join("track.mp3");
        fs::write(&path, bytes).unwrap();

        assert!(
            crate::tag_source::needs_repair(&path),
            "the stray null flags"
        );
        commit(&path, &[]).unwrap();
        assert!(
            !crate::tag_source::needs_repair(&path),
            "the rewrite clears it"
        );
        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Title).as_deref(), Some(title));
        assert!(fs::read(&path).unwrap().ends_with(&mpeg_audio()));
    }

    /// A minimal JPEG-shaped blob: the magic the art sniffer keys on, so
    /// the mime rescues to image/jpeg no matter what the tag declares.
    fn jpeg(marker: u8) -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, marker, 0x2A, 0xFF, 0xD9]
    }

    fn set_pic(kind: PicKind, bytes: Vec<u8>) -> PicChange {
        PicChange {
            kind,
            data: Some((bytes, "image/jpeg".into())),
        }
    }

    /// A cover set, read back, then replaced and removed, on both formats:
    /// the write lands the picture at its slot, a second write swaps it,
    /// and a remove clears it, all over untouched audio.
    #[test]
    fn cover_set_replace_remove_round_trips() {
        let dir = scratch("covers");
        for (path, audio) in [
            (mp3_file(&dir, "track.mp3"), mpeg_audio()),
            (
                flac_file(&dir, "track.flac"),
                (0..600u32).map(|i| (i * 11 % 253) as u8).collect(),
            ),
        ] {
            let front = jpeg(0x11);
            commit_with(&path, &[], &[set_pic(PicKind::Front, front.clone())]).unwrap();
            let pics = read_pictures(&path).unwrap();
            assert_eq!(pics.len(), 1);
            assert_eq!(pics[0].0, PicKind::Front);
            assert_eq!(pics[0].1, front);
            assert!(fs::read(&path).unwrap().ends_with(&audio), "audio survives");

            // A back cover joins it, then the front is swapped.
            let back = jpeg(0x22);
            let front2 = jpeg(0x33);
            commit_with(
                &path,
                &[],
                &[
                    set_pic(PicKind::Back, back.clone()),
                    set_pic(PicKind::Front, front2.clone()),
                ],
            )
            .unwrap();
            let pics = read_pictures(&path).unwrap();
            assert_eq!(pics.len(), 2);
            let of = |kind| {
                pics.iter()
                    .find(|(k, _, _)| *k == kind)
                    .map(|(_, d, _)| d.clone())
            };
            assert_eq!(of(PicKind::Front).as_deref(), Some(front2.as_slice()));
            assert_eq!(of(PicKind::Back).as_deref(), Some(back.as_slice()));

            // The front comes off, the back stays.
            commit_with(
                &path,
                &[],
                &[PicChange {
                    kind: PicKind::Front,
                    data: None,
                }],
            )
            .unwrap();
            let pics = read_pictures(&path).unwrap();
            assert_eq!(pics.len(), 1);
            assert_eq!(pics[0].0, PicKind::Back);
            assert!(fs::read(&path).unwrap().ends_with(&audio), "audio survives");
        }
    }

    /// A cover replace on the Bandcamp unsync shape: the mangled front is
    /// what the edit overwrites, so this is the repair the rescue path
    /// makes explicit, and the new bytes read back clean.
    #[test]
    fn cover_replace_on_unsync_mp3() {
        let image = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0x00, 0x59, 0xFF, 0xFF, 0xD9,
        ];
        let mut body = vec![0x00];
        body.extend(b"image/jpeg\0");
        body.push(3); // front cover
        body.extend(b"c\0");
        body.extend(image);
        let stored = stuff(&body);
        let mut frame = b"APIC".to_vec();
        frame.extend(synch(stored.len() as u32 + 4));
        frame.extend([0x00, 0x03]);
        frame.extend(synch(body.len() as u32));
        frame.extend(&stored);
        let mut tag = b"ID3\x04\x00\x80".to_vec();
        tag.extend(synch(frame.len() as u32));
        tag.extend(&frame);

        let dir = scratch("unsync-cover");
        let path = dir.join("track.mp3");
        let mut bytes = tag;
        bytes.extend(mpeg_audio());
        fs::write(&path, bytes).unwrap();

        let new = jpeg(0x44);
        commit_with(&path, &[], &[set_pic(PicKind::Front, new.clone())]).unwrap();
        let (cover, mime) = crate::art::cover_art(&path).expect("the new cover resolves");
        assert_eq!(cover, new);
        assert_eq!(mime, "image/jpeg");
        assert!(fs::read(&path).unwrap().ends_with(&mpeg_audio()));
    }

    /// The untyped-cover shape a lot of taggers (Windows Media Player among
    /// them) write: an ID3v2.3 APIC typed `Other` (0), not front. The front
    /// slot must fold it in, and replacing the front must consolidate onto
    /// one typed cover rather than orphan the untyped one beside it.
    #[test]
    fn front_slot_owns_an_untyped_cover() {
        let image = jpeg(0x55);
        let mut body = vec![0x00];
        body.extend(b"image/jpeg\0");
        body.push(0); // picture type Other
        body.push(0); // empty description
        body.extend(&image);
        let mut frame = b"APIC".to_vec();
        frame.extend((body.len() as u32).to_be_bytes()); // v2.3: plain size
        frame.extend([0x00, 0x00]);
        frame.extend(&body);
        let mut tag = b"ID3\x03\x00\x00".to_vec();
        tag.extend(synch(frame.len() as u32));
        tag.extend(&frame);

        let dir = scratch("untyped-cover");
        let path = dir.join("track.mp3");
        let mut bytes = tag;
        bytes.extend(mpeg_audio());
        fs::write(&path, bytes).unwrap();

        // The untyped picture reads back as the front slot.
        let pics = read_pictures(&path).unwrap();
        assert_eq!(pics.len(), 1);
        assert_eq!(pics[0].0, PicKind::Front);
        assert_eq!(pics[0].1, image);

        // Replacing the front leaves exactly one cover, the new typed one.
        let new = jpeg(0x66);
        commit_with(&path, &[], &[set_pic(PicKind::Front, new.clone())]).unwrap();
        let pics = read_pictures(&path).unwrap();
        assert_eq!(pics.len(), 1, "the untyped cover must not orphan");
        assert_eq!(pics[0].1, new);
        assert_eq!(crate::art::cover_art(&path).unwrap().0, new);
    }
}
