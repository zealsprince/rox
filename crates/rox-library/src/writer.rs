//! The metadata writer per ADR 4: tag writes through lofty, wrapped in a
//! copy-verify-rename layer. lofty rewrites files in place and a failed write
//! can leave one unrecoverable, so the original is never written to: a commit
//! clones the file, writes and verifies the clone, and renames it over the
//! original. Blocking file IO; run it off the UI thread.
//!
//! The standard fields go through lofty's SplitTag/MergeTag, which passes
//! frames it doesn't understand through untouched. Custom fields go through
//! the format types directly (TXXX, Vorbis keys, MP4 atoms), since ItemKey has
//! no slot for them.
//!
//! An ID3v2.4 tag whose header and APIC both flag unsynchronisation reads back
//! mangled through lofty 0.24 (see the art module), so that picture is re-read
//! raw and carried through every commit. The raw path recovers one picture,
//! so a multi-picture tag in that shape fails verification.

use std::borrow::Cow;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst, Mp4File};
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

/// A tag field the editor can address. `Custom` is a TXXX description or
/// Vorbis key written through the format tag. `Rating` is the 0-10 display
/// number and fans out to POPM/RATING plus FMPS_Rating on write.
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
    /// The four sort names: the Latin form a library orders by when the
    /// displayed name isn't one.
    TitleSort,
    ArtistSort,
    AlbumArtistSort,
    AlbumSort,
    /// USLT on ID3v2, UNSYNCEDLYRICS on Vorbis. May hold LRC timestamps.
    Lyrics,
    Rating,
    /// Written by [`commit_replay_gain`]. lofty matches the keys
    /// case-insensitively, so a set replaces a differently cased frame.
    ReplayGain(GainKind),
    Custom(String),
    /// A tag outside the editable set, by its [`read_unknown`] key. A set writes
    /// back through the key's own carrier so it never leaves a TXXX twin.
    Unknown(String),
}

/// Named for [`crate::replaygain::ReplayGain`]'s fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainKind {
    TrackDb,
    TrackPeak,
    AlbumDb,
    AlbumPeak,
}

impl GainKind {
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

/// A picture slot the cover editor addresses. Other lofty picture types pass
/// through every commit untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicKind {
    Front,
    Back,
    Media,
    Artist,
}

impl PicKind {
    fn primary_type(self) -> PictureType {
        match self {
            PicKind::Front => PictureType::CoverFront,
            PicKind::Back => PictureType::CoverBack,
            PicKind::Media => PictureType::Media,
            PicKind::Artist => PictureType::Artist,
        }
    }

    /// Every lofty type this slot owns. Front also owns the untyped `Other`, where
    /// plenty of taggers (Windows Media Player among them) store the cover.
    fn owned_types(self) -> &'static [PictureType] {
        match self {
            PicKind::Front => &[PictureType::CoverFront, PictureType::Other],
            PicKind::Back => &[PictureType::CoverBack],
            PicKind::Media => &[PictureType::Media],
            PicKind::Artist => &[PictureType::Artist],
        }
    }

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

/// `data` `None` removes the slot's picture.
#[derive(Clone, Debug)]
pub struct PicChange {
    pub kind: PicKind,
    pub data: Option<(Vec<u8>, String)>,
}

pub struct Edit {
    pub path: PathBuf,
    pub changes: Vec<Change>,
    pub pictures: Vec<PicChange>,
}

/// `Year` writes the recording date key, the one the scanner's `date()` reads
/// first.
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
        Field::TitleSort => ItemKey::TrackTitleSortOrder,
        Field::ArtistSort => ItemKey::TrackArtistSortOrder,
        Field::AlbumArtistSort => ItemKey::AlbumArtistSortOrder,
        Field::AlbumSort => ItemKey::AlbumTitleSortOrder,
        // lofty refuses ItemKey::Lyrics on ID3v2, so always the unsynchronised key.
        Field::Lyrics => ItemKey::UnsyncLyrics,
        Field::ReplayGain(kind) => kind.item_key(),
        // `apply_rating` writes the rating's popularimeter form itself.
        Field::Rating | Field::Custom(_) | Field::Unknown(_) => return None,
    })
}

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
        ItemKey::TrackTitleSortOrder => Field::TitleSort,
        ItemKey::TrackArtistSortOrder => Field::ArtistSort,
        ItemKey::AlbumArtistSortOrder => Field::AlbumArtistSort,
        ItemKey::AlbumTitleSortOrder => Field::AlbumSort,
        ItemKey::UnsyncLyrics | ItemKey::Lyrics => Field::Lyrics,
        // ReplayGain is a measurement, so it stays out of the editor.
        _ => return None,
    })
}

/// A file's editable fields: the named set, then the format's customs.
/// Everything else passes through commits untouched. A parser panic costs an
/// error, never the process.
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
                    // Acoustic vectors stay out of the editor: a screenful of base64.
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
                if key.eq_ignore_ascii_case(rating::FMPS_KEY)
                    || key
                        .get(..7)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("RATING:"))
                {
                    continue;
                }
                if embed_tag::is_key(key) {
                    continue;
                }
                if ItemKey::from_key(lofty::tag::TagType::VorbisComments, key).is_none() {
                    out.push((Field::Custom(key.to_string()), value.to_string()));
                }
            }
        }
        FileType::Mp4 => {
            // The split leaves exactly the atoms lofty had no key for.
            let (remainder, generic) = parse_mp4(path)?
                .ilst()
                .cloned()
                .unwrap_or_default()
                .split_tag();
            named_fields(generic, &mut out);
            for atom in &*remainder {
                let key = ilst_key(atom.ident());
                // The rating matches off the atom name because its mean is the tagger's
                // choice.
                if is_fmps_atom(atom.ident()) || embed_tag::is_key(&key) {
                    continue;
                }
                for text in atom.data().filter_map(atom_text) {
                    out.push((Field::Custom(key.clone()), text));
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

/// Binary frames (PRIV, GEOB, UFID) are never decoded, only sized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnknownValue {
    Text(String),
    Binary(usize),
}

impl UnknownValue {
    pub fn display(&self) -> String {
        match self {
            UnknownValue::Text(text) => text.clone(),
            UnknownValue::Binary(bytes) => format!("{} binary", human_bytes(*bytes)),
        }
    }
}

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

/// ReplayGain is named here because MP3 surfaces it as TXXX while FLAC has
/// lofty map it, and the list has to hide it on both.
fn unknown_excluded(key: &str) -> bool {
    // A foreign-mean FMPS atom arrives as "mean:FMPS_Rating"; match the name
    // alone, like [`rating::from_ilst`].
    let name = key.rsplit(':').next().unwrap_or(key);
    if name.eq_ignore_ascii_case(rating::FMPS_KEY) || embed_tag::is_key(key) {
        return true;
    }
    ["RATING:", "REPLAYGAIN_"].iter().any(|prefix| {
        key.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

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

/// A file's tags the editor has no row for: the format's customs, the items
/// lofty maps but rox has no field for, and binary frames. Edits go through
/// [`Field::Unknown`] by key. A parser panic costs an error.
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
                    // A file can hold several PRIVs, told apart by owner.
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
                    // Pictures and a bare popularimeter have their own editors; the rest hold
                    // structure a one-line row would lie about.
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
        FileType::Mp4 => {
            let (remainder, generic) = parse_mp4(path)?
                .ilst()
                .cloned()
                .unwrap_or_default()
                .split_tag();
            mapped_unknowns(&generic, lofty::tag::TagType::Mp4Ilst, None, &mut out);
            for atom in &*remainder {
                let key = ilst_key(atom.ident());
                for value in atom.data().filter_map(atom_unknown) {
                    push_unknown(&mut out, key.clone(), value);
                }
            }
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
    out.retain(|(key, _)| !unknown_excluded(key));
    Ok(out)
}

/// Mapped items with no rox field, labeled by the key the format writes.
///
/// `vendor` is the FLAC vendor string, which the Vorbis split injects as an
/// EncoderSoftware item even when the file has no such tag.
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
        // Fold to [`ilst_key`]'s form so one label addresses a tag either way.
        let label = match key.map_key(tag_type) {
            Some(mapped) if tag_type == lofty::tag::TagType::Mp4Ilst => ilst_label(mapped),
            Some(mapped) => mapped.to_string(),
            None => format!("{key:?}"),
        };
        push_unknown(out, label, value);
    }
}

/// Repeated text keys fold into a "; " list. Binary frames stay separate rows.
fn push_unknown(out: &mut Vec<(String, UnknownValue)>, key: String, value: UnknownValue) {
    if let UnknownValue::Text(text) = &value {
        let prior = out
            .iter_mut()
            .find(|(k, v)| k == &key && matches!(v, UnknownValue::Text(_)));

        if let Some((_, UnknownValue::Text(existing))) = prior {
            existing.push_str("; ");
            existing.push_str(text);
            return;
        }
    }

    out.push((key, value));
}

/// The editor asks this before blaming a read failure on the file. Also false
/// for a fragmented MP4 placed by absolute offsets (see [`file_type`]).
pub fn supported(path: &Path) -> bool {
    file_type(path).is_ok()
}

/// Genre folds its items into one "; " list at the first item's position.
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

fn embedded_pictures(
    path: &Path,
    kind: FileType,
) -> Result<Vec<(PictureType, Vec<u8>, String)>, String> {
    Ok(match kind {
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
        // FLAC pictures live in PICTURE blocks off the vorbis comments.
        FileType::Flac => parse_flac(path)?
            .pictures()
            .iter()
            .map(|(picture, _)| pic_tuple(picture))
            .collect(),
        // MP4 has no picture type, so every `covr` reads back as `Other`, which the
        // front slot owns.
        FileType::Mp4 => parse_mp4(path)?
            .ilst()
            .cloned()
            .unwrap_or_default()
            .split_tag()
            .1
            .pictures()
            .iter()
            .map(pic_tuple)
            .collect(),
        _ => unreachable!("file_type only passes writable formats"),
    })
}

/// The mime is sniffed off the magic bytes when the tag's is missing or unknown.
fn pic_tuple(picture: &Picture) -> (PictureType, Vec<u8>, String) {
    let mime = match picture.mime_type() {
        Some(MimeType::Unknown(_)) | None => {
            art::sniff(picture.data()).unwrap_or_default().to_string()
        }
        Some(mime) => mime.as_str().to_string(),
    };
    (picture.pic_type(), picture.data().to_vec(), mime)
}

/// Unslotted picture types are left out here but pass through commits.
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
    // Show the rescued front cover so the diff sees the real image.
    if kind == FileType::Mpeg
        && let Some(front) = out.iter_mut().find(|(k, _, _)| *k == PicKind::Front)
        && let Some((data, mime)) = art::unsync_apic(path, art::ArtKind::Front)
    {
        front.1 = data;
        front.2 = mime;
    }
    Ok(out)
}

/// Commit through the atomic layer: clone, write, verify (every change reads
/// back, pictures byte-identical, audio hash unchanged), rename. Any failure,
/// including a parser panic, unlinks the clone and leaves the original
/// byte-identical.
pub fn commit(path: &Path, changes: &[Change]) -> Result<(), String> {
    commit_with(path, changes, &[])
}

/// [`commit`] with picture edits alongside the field changes.
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

/// Only sub 0 is writable. A cue track shares its image with the whole disc,
/// so a title or rating written there would apply to every track.
pub fn writes_to_file(sub: u16) -> bool {
    sub == 0
}

/// [`commit_with`] that skips the file write for a cue track and returns Ok,
/// so the edit stays in the library.
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

/// One malformed file costs its own entry, never the batch.
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

/// Write the four ReplayGain numbers into a file's tags, ADR 19's opt-in.
///
/// A `None` removes that item, so a re-measure can't leave last time's album
/// numbers beside a new track figure. Non-finite values are dropped rather
/// than written.
pub fn commit_replay_gain(path: &Path, gain: ReplayGain) -> Result<(), String> {
    commit(path, &replay_gain_changes(gain))
}

/// Write one model's acoustic vector into a file's tags, so a wiped library
/// gets its descriptions back without decoding again. MP3 and FLAC only (see
/// [`crate::embed_tag::writable`]).
pub fn commit_embedding(path: &Path, model: &str, vec: &[f32]) -> Result<(), String> {
    commit(
        path,
        &[Change {
            field: Field::Custom(embed_tag::key(model)),
            value: Some(embed_tag::encode(vec)),
        }],
    )
}

/// The four values as sets alone, clears dropped. What [`crate::bake`]
/// writes: an empty slot in a stored row means the database never held that
/// number, and clearing the file's value over it would delete metadata.
pub fn replay_gain_additions(gain: ReplayGain) -> Vec<Change> {
    replay_gain_changes(gain)
        .into_iter()
        .filter(|change| change.value.is_some())
        .collect()
}

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

/// Six decimals, the form the RG spec asks for.
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
    let audio_hash = hash_span(path, audio_span(path, kind)?)?;
    let rescue = if kind == FileType::Mpeg {
        art::unsync_apic(path, art::ArtKind::Front)
    } else {
        None
    };
    // MP3 always verifies pictures (the unsync hazard); FLAC and MP4 only when an
    // edit touches them.
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

    // Flush before the rename, or a power cut can leave a truncated file. Needs
    // write access: Windows' FlushFileBuffers rejects a read-only handle.
    fs::OpenOptions::new()
        .write(true)
        .open(tmp)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync clone: {e}"))?;
    fs::rename(tmp, path).map_err(|e| format!("rename over original: {e}"))
}

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
            // Padding outside the declared tag size makes lofty's write probe give up
            // before it finds the audio. Fold it into the tag first.
            fold_tag_gap(&mut file)?;
            // Read through the sanitiser so a double-unsynced tag parses clean.
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
            // After the rescue, so a front-cover edit wins over the rescued one.
            apply_pictures(&mut generic, pictures);
            let mut tag = remainder.merge_tag(generic);
            // lofty keeps the read tag's unsync header flag but writes raw frames, so
            // the next read would collapse byte pairs that were never stuffed. Clear it.
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
        FileType::Mp4 => {
            let mut mp4 = parse_mp4(tmp)?;
            let mut tag = mp4.ilst().cloned().unwrap_or_default();
            for change in changes {
                match &change.field {
                    Field::Custom(key) => {
                        let ident = ilst_ident(&tag, key);
                        // Remove every atom the key addresses, so a rating under two means collapses
                        // to one.
                        tag.remove(&ident).for_each(drop);
                        if let Some(v) = &change.value {
                            tag.insert(Atom::new(ident, AtomData::UTF8(v.clone())));
                        }
                    }
                    Field::Unknown(key) => apply_unknown_mp4(&mut tag, key, &change.value),
                    _ => {}
                }
            }
            let (remainder, mut generic) = tag.split_tag();
            apply_unknown_generic(&mut generic, lofty::tag::TagType::Mp4Ilst, changes);
            apply_named(&mut generic, changes);
            // No [`apply_rating`] on MP4: lofty maps the popularimeter to the `rate`
            // atom, which no player reads as stars. FMPS carries the rating alone here.
            apply_pictures(&mut generic, pictures);
            mp4.set_ilst(remainder.merge_tag(generic));
            file.rewind().map_err(|e| format!("rewind: {e}"))?;
            mp4.save_to(&mut file, WriteOptions::default())
                .map_err(|e| format!("write: {e}"))
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
}

/// A genre set splits its "; " list into one item per value, so the merge
/// writes the format's native multiples.
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
        // MP4's `©lyr` maps to the plain Lyrics key while the others use the
        // unsynchronised one. Remove the plain key too, or a set leaves a twin and a
        // clear gets written straight back.
        if change.field == Field::Lyrics {
            generic.remove_key(ItemKey::Lyrics);
        }
        match &change.value {
            Some(v) => drop(generic.insert_text(key, v.clone())),
            None => generic.remove_key(key),
        }
    }
}

/// The key [`read_unknown`] files an ID3v2 frame under.
fn mpeg_unknown_key(frame: &Frame<'_>) -> String {
    match frame {
        Frame::UserText(f) => f.description.to_string(),
        Frame::UserUrl(f) => f.description.to_string(),
        Frame::Private(f) => format!("PRIV:{}", f.owner),
        Frame::UniqueFileIdentifier(f) => format!("UFID:{}", f.owner),
        _ => frame.id_str().to_string(),
    }
}

const ITUNES_MEAN: &str = "com.apple.iTunes";

/// The key [`read`] and [`read_unknown`] file an MP4 atom under.
///
/// A fourcc renders its leading `0xA9` as ©. A `com.apple.iTunes` freeform
/// reads as its bare name, any other mean as `mean:name`. The bare form is
/// what lets the rating, ReplayGain and vector exclusions match one spelling
/// on all three formats.
fn ilst_key(ident: &AtomIdent<'_>) -> String {
    match ident {
        AtomIdent::Fourcc(fourcc) => fourcc.iter().copied().map(char::from).collect(),
        AtomIdent::Freeform { mean, name } if mean == ITUNES_MEAN => name.to_string(),
        AtomIdent::Freeform { mean, name } => format!("{mean}:{name}"),
    }
}

/// Matches on the freeform name alone, like [`rating::from_ilst`]. Matching
/// the whole key leaks foreign-mean atoms into the custom rows and writes a
/// `com.apple.iTunes` twin on the next rating edit.
fn is_fmps_atom(ident: &AtomIdent<'_>) -> bool {
    matches!(
        ident,
        AtomIdent::Freeform { name, .. } if name.eq_ignore_ascii_case(rating::FMPS_KEY)
    )
}

fn ilst_addresses(ident: &AtomIdent<'_>, key: &str) -> bool {
    ilst_key(ident) == key || (key.eq_ignore_ascii_case(rating::FMPS_KEY) && is_fmps_atom(ident))
}

/// The inverse of [`ilst_key`]. An atom already filed under the key keeps its
/// identity. A plain four-letter key becomes a freeform, since it's likelier
/// someone's own key than an unmapped atom.
fn ilst_ident(tag: &Ilst, key: &str) -> AtomIdent<'static> {
    if let Some(existing) = tag
        .into_iter()
        .find(|atom| ilst_addresses(atom.ident(), key))
    {
        return existing.ident().clone().into_owned();
    }
    if let Some((mean, name)) = key.split_once(':') {
        return AtomIdent::Freeform {
            mean: Cow::Owned(mean.to_string()),
            name: Cow::Owned(name.to_string()),
        };
    }
    if let Some(fourcc) = fourcc_key(key) {
        return AtomIdent::Fourcc(fourcc);
    }
    AtomIdent::Freeform {
        mean: Cow::Borrowed(ITUNES_MEAN),
        name: Cow::Owned(key.to_string()),
    }
}

/// Latin-1 both ways, matching lofty's own `c as u8`.
fn fourcc_key(key: &str) -> Option<[u8; 4]> {
    if !key.contains('©') {
        return None;
    }
    let mut out = [0u8; 4];
    let mut chars = key.chars();
    for slot in &mut out {
        *slot = u8::try_from(u32::from(chars.next()?)).ok()?;
    }
    chars.next().is_none().then_some(out)
}

/// MP4 keys get their `----:mean:name` form back before the lookup.
fn mapped_key(tag_type: lofty::tag::TagType, key: &str) -> Option<ItemKey> {
    if tag_type != lofty::tag::TagType::Mp4Ilst {
        return ItemKey::from_key(tag_type, key);
    }
    ItemKey::from_key(tag_type, key)
        .or_else(|| ItemKey::from_key(tag_type, &format!("----:{ITUNES_MEAN}:{key}")))
        .or_else(|| ItemKey::from_key(tag_type, &format!("----:{key}")))
}

fn ilst_label(mapped: &str) -> String {
    let Some(freeform) = mapped.strip_prefix("----:") else {
        return mapped.to_string();
    };
    freeform
        .strip_prefix(ITUNES_MEAN)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(freeform)
        .to_string()
}

fn atom_text(data: &AtomData) -> Option<String> {
    match data {
        AtomData::UTF8(text) | AtomData::UTF16(text) => Some(text.clone()),
        _ => None,
    }
}

fn atom_unknown(data: &AtomData) -> Option<UnknownValue> {
    Some(match data {
        AtomData::UTF8(text) | AtomData::UTF16(text) => UnknownValue::Text(text.clone()),
        AtomData::SignedInteger(n) => UnknownValue::Text(n.to_string()),
        AtomData::UnsignedInteger(n) => UnknownValue::Text(n.to_string()),
        AtomData::Bool(flag) => UnknownValue::Text(u8::from(*flag).to_string()),
        AtomData::Unknown { data, .. } => UnknownValue::Binary(data.len()),
        _ => return None,
    })
}

/// Every atom the key names goes. A mapped set waits for
/// [`apply_unknown_generic`] so it writes through the file's own atom.
fn apply_unknown_mp4(tag: &mut Ilst, key: &str, value: &Option<String>) {
    let ident = ilst_ident(tag, key);
    tag.retain(|atom| !ilst_addresses(atom.ident(), key));
    if let Some(v) = value
        && mapped_key(lofty::tag::TagType::Mp4Ilst, key).is_none()
    {
        tag.insert(Atom::new(ident, AtomData::UTF8(v.clone())));
    }
}

/// Every frame the key names goes. A mapped set waits for
/// [`apply_unknown_generic`] so it writes through the format's own frame.
fn apply_unknown_mpeg(tag: &mut Id3v2Tag, key: &str, value: &Option<String>) {
    tag.retain(|frame| mpeg_unknown_key(frame) != key);
    if let Some(v) = value
        && mapped_key(lofty::tag::TagType::Id3v2, key).is_none()
    {
        drop(tag.insert_user_text(key.to_string(), v.clone()));
    }
}

fn apply_unknown_generic(generic: &mut Tag, tag_type: lofty::tag::TagType, changes: &[Change]) {
    for change in changes {
        let Field::Unknown(key) = &change.field else {
            continue;
        };
        let Some(v) = &change.value else { continue };
        if let Some(item_key) = mapped_key(tag_type, key) {
            generic.insert_text(item_key, v.clone());
        }
    }
}

/// Fan a rating change out into its normalized value plus the exact FMPS
/// custom. The whole-star half goes through [`apply_rating`].
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

/// Whole-star popularimeter with an empty email, which lofty merges to a bare
/// POPM or RATING. A set replaces every popularimeter, whoever wrote it.
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

/// lofty's Vorbis split passes a bare RATING through but its merge only writes
/// the email|stars|counter form, so a commit would drop another app's rating.
/// Reformat it at whole-star resolution when this commit brings no rating.
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

/// Swap the rescued bytes in for the mangled front cover, or the first picture
/// failing that.
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

/// [`expected_pictures`] must match this exactly, or verify fails.
fn apply_pictures(generic: &mut Tag, pictures: &[PicChange]) {
    for change in pictures {
        // Drop every owned type first; [`expected_pictures`] does the same.
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

/// lofty holds FLAC pictures off the vorbis comments, hence the separate path.
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
            // A picture that won't parse still writes with a zeroed info block.
            let info = PictureInformation::from_picture(&picture).unwrap_or_default();
            let _ = flac.insert_picture(picture, Some(info));
        }
    }
}

/// Every change read back off the clone through the same path the next scan
/// takes.
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
        FileType::Mp4 => {
            let tag = parse_mp4(tmp)?.ilst().cloned().unwrap_or_default();
            let customs = custom_keys
                .map(|key| {
                    let value = tag
                        .get(&ilst_ident(&tag, &key))
                        .and_then(|atom| atom.data().find_map(atom_text));
                    (key, value)
                })
                .collect();
            (tag.split_tag().1, customs)
        }
        _ => unreachable!("file_type only passes writable formats"),
    };
    let unknowns: Vec<(String, UnknownValue)> =
        if changes.iter().any(|c| matches!(c.field, Field::Unknown(_))) {
            read_unknown_inner(tmp)?
        } else {
            Vec::new()
        };
    for change in changes {
        // The rating verifies at star resolution; the exact value verifies through
        // its FMPS custom.
        if change.field == Field::Rating {
            // MP4 wrote no star form, so nothing to read back here.
            if kind == FileType::Mp4 {
                continue;
            }
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
        // Genre verifies as a canonical "; " list on both sides.
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
            named => read_named(&generic, named),
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

/// Checks both lyrics keys: MP4's `©lyr` reads back as plain `Lyrics`.
fn read_named(generic: &Tag, field: &Field) -> Option<String> {
    let key = item_key(field).expect("named fields have keys");
    generic
        .get_string(key)
        .or_else(|| match field {
            Field::Lyrics => generic.get_string(ItemKey::Lyrics),
            _ => None,
        })
        .map(str::to_string)
}

/// The pictures the clone must hold. Mirrors [`set_front_picture`] and
/// [`apply_pictures`] step for step.
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

/// Byte multisets: the write may reorder frames but only touch images an edit
/// named.
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

/// The formats the writer handles, off the file's content.
fn file_type(path: &Path) -> Result<FileType, String> {
    let kind = Probe::open(path)
        .map_err(|e| format!("open: {e}"))?
        .guess_file_type()
        .map_err(|e| format!("probe: {e}"))?
        .file_type()
        .ok_or_else(|| format!("unrecognized format: {}", path.display()))?;
    match kind {
        FileType::Mpeg | FileType::Flac => Ok(kind),
        // A fragmented MP4 whose fragments count from their own `moof` writes fine:
        // a resized tag shifts every fragment equally (checked on a real 42-fragment
        // DASH file, decoded identically through symphonia and ffmpeg).
        //
        // Refuse fragments placed by absolute position, or a `sidx`. lofty patches
        // the base data offset of one `moof` only and never a `sidx`, so the offsets
        // go stale while every hashed byte stays the same: verify passes and the file
        // is silently unplayable.
        FileType::Mp4 if crate::mp4::has_absolute_fragment_offsets(path) => Err(format!(
            "writing tags into a fragmented MP4 with absolute offsets is not supported: {}",
            path.display()
        )),
        FileType::Mp4 => Ok(kind),
        other => Err(format!("writing {other:?} tags is not supported yet")),
    }
}

/// Grow the ID3v2 size field over junk between the tag end and the first MPEG
/// sync. lofty re-detects the format mid-save and gives up after 1024 junk
/// bytes, so the junk breaks every write. Clone only, and only when a sync
/// follows.
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

/// For the repair scan: `Err` holds the parse error. Unsupported formats read
/// as fine.
pub fn readable(path: &Path) -> Result<(), String> {
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(kind) = file_type(path) else {
            return Ok(());
        };
        match kind {
            FileType::Mpeg => parse_mpeg(path).map(drop),
            FileType::Flac => parse_flac(path).map(drop),
            FileType::Mp4 => parse_mp4(path).map(drop),
            _ => Ok(()),
        }
    }))
    .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())))
}

/// Tags only, so a file with a garbled stream still gets its tags fixed.
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

/// No ID3v2 sanitising: [`crate::tag_source`] repairs a shape only ID3v2 has.
fn parse_mp4(path: &Path) -> Result<Mp4File, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    Mp4File::read_from(&mut file, parse_opts()).map_err(|e| format!("parse: {e}"))
}

/// Public so the library watcher can ignore the writer's own clone traffic.
pub const CLONE_SUFFIX: &str = ".rox-write";

pub fn is_clone_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(CLONE_SUFFIX))
}

/// A sibling, so the rename never crosses a filesystem.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(CLONE_SUFFIX);
    path.with_file_name(name)
}

/// The byte ranges holding the audio stream, so their hash proves the write
/// only moved tags.
///
/// A list because of MP4: `mdat` can sit either side of `moov`, and a
/// fragmented file has an `mdat` per fragment. The `moof` payloads are
/// included, since a fragment is only safe to shift if its header is untouched.
fn audio_span(path: &Path, kind: FileType) -> Result<Vec<(u64, u64)>, String> {
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
            // Hash from the first sync, the boundary the fold uses, so junk a repair
            // write drops doesn't change the span.
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
                    // The size counts the items and footer; a flagged header adds 32.
                    let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
                    let flags = u32::from_le_bytes(footer[20..24].try_into().unwrap());
                    let header = if flags & (1 << 31) != 0 { 32 } else { 0 };
                    end = end.saturating_sub(size + header);
                }
            }
            Ok(vec![(start.min(len), end.max(start.min(len)))])
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
            Ok(vec![(pos.min(len), len)])
        }
        FileType::Mp4 => {
            // Recomputed on the clone, so a grown tag can move the audio and still hash
            // equal. No `mdat` at all is an error, or verify would pass anything it
            // couldn't parse.
            let spans = crate::mp4::stream_spans(path)
                .ok_or_else(|| format!("no mp4 audio to hash: {}", path.display()))?;
            Ok(spans
                .into_iter()
                .map(|span| (span.start.min(len), span.end.min(len)))
                .collect())
        }
        _ => unreachable!("file_type only passes writable formats"),
    }
}

/// FNV-1a with one state across all spans, so order and boundaries are hashed
/// too. Guards against a moved boundary, not an adversary.
fn hash_span(path: &Path, spans: Vec<(u64, u64)>) -> Result<u64, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut buf = [0u8; 64 * 1024];
    for (start, end) in spans {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("seek: {e}"))?;
        let mut remaining = end.saturating_sub(start);
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
    }
    Ok(hash)
}

/// Tag fixtures shared with the modules that write through this one.
#[cfg(test)]
pub(crate) use tests::{flac_file, m4a_file, mp3_file, scratch};

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

    /// Three MPEG1 Layer3 frames with patterned payloads, so a moved span can't
    /// hash the same.
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

    fn m4a_atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn m4a_audio() -> Vec<u8> {
        (0..1024u32).map(|i| (i * 13 % 251) as u8).collect()
    }

    fn m4a_mvhd() -> Vec<u8> {
        let mut payload = vec![0u8; 100];
        payload[12..16].copy_from_slice(&44_100u32.to_be_bytes());
        m4a_atom(b"mvhd", &payload)
    }

    /// `moov` sits in front of the audio, the real-world layout, so a growing tag
    /// pushes the audio down and the span hash is actually tested.
    fn m4a_bytes(moov_children: &[Vec<u8>]) -> Vec<u8> {
        let mut moov = Vec::new();
        for child in moov_children {
            moov.extend_from_slice(child);
        }
        let mut out = m4a_atom(b"ftyp", b"M4A \0\0\0\0M4A mp42isom");
        out.extend(m4a_atom(b"moov", &moov));
        out.extend(m4a_atom(b"mdat", &m4a_audio()));
        out
    }

    pub(crate) fn m4a_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, m4a_bytes(&[m4a_mvhd()])).unwrap();
        path
    }

    /// Flag 0x02_0000 is default-base-is-moof (DASH); flag 1 is an absolute base
    /// data offset, eight bytes after.
    fn m4a_moof(tfhd_flags: u32) -> Vec<u8> {
        let mut tfhd = tfhd_flags.to_be_bytes().to_vec();
        tfhd.extend_from_slice(&1u32.to_be_bytes());
        if tfhd_flags & 1 != 0 {
            tfhd.extend_from_slice(&[0u8; 8]);
        }
        m4a_atom(b"moof", &m4a_atom(b"traf", &m4a_atom(b"tfhd", &tfhd)))
    }

    fn m4a_fragmented_bytes(tfhd_flags: u32) -> Vec<u8> {
        let mvex = m4a_atom(b"mvex", &m4a_atom(b"mehd", &[0, 0, 0, 0, 0, 1, 0, 0]));
        let mut moov = m4a_mvhd();
        moov.extend(mvex);
        let mut out = m4a_atom(b"ftyp", b"isom\0\0\0\0iso5");
        out.extend(m4a_atom(b"moov", &moov));
        out.extend(m4a_moof(tfhd_flags));
        out.extend(m4a_atom(b"mdat", &m4a_audio()));
        out
    }

    fn atoms_under(path: &Path, key: &str) -> Vec<String> {
        parse_mp4(path)
            .unwrap()
            .ilst()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|atom| ilst_key(atom.ident()) == key)
            .flat_map(|atom| atom.into_data())
            .filter_map(|data| atom_text(&data))
            .collect()
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

    #[test]
    fn mp3_unknown_tags_cover_the_three_tiers() {
        use lofty::TextEncoding;
        use lofty::id3::v2::{
            BinaryFrame, ExtendedTextFrame, FrameId, Id3v2Tag, PrivateFrame, TextInformationFrame,
        };
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
        tag.insert(txxx("MY NOTE", "kept"));
        tag.insert(Frame::Text(TextInformationFrame::new(
            FrameId::Valid(Cow::Borrowed("TBPM")),
            TextEncoding::UTF8,
            "128",
        )));
        tag.insert(txxx("MusicBrainz Artist Id", "f4ab-1"));
        tag.insert(txxx("REPLAYGAIN_TRACK_GAIN", "-7.35 dB"));
        tag.insert(txxx(rating::FMPS_KEY, "0.8"));
        tag.insert(txxx(&embed_tag::key("test-model"), "v1;dim=2;f16;AAAA"));
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

    /// lofty maps ReplayGain on FLAC but leaves it as TXXX on MP3, so only the
    /// explicit exclusion keeps the two lists alike.
    #[test]
    fn flac_unknown_tags_cover_the_tiers_and_exclusions() {
        use lofty::ogg::VorbisComments;

        let dir = scratch("flac-unknown");
        let path = flac_file(&dir, "track.flac");
        let mut tag = VorbisComments::default();
        tag.push("TITLE".into(), "Known".into());
        tag.push("MY NOTE".into(), "kept".into());
        tag.push("BPM".into(), "128".into());
        tag.push("MUSICBRAINZ_ARTISTID".into(), "f4ab-1".into());
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
            "ENCODER",
        ] {
            assert!(
                unknown_of(&rows, key).is_none(),
                "{key} must stay out of the unknown list"
            );
        }
    }

    #[test]
    fn mp3_unknown_edits_address_every_tier() {
        use lofty::TextEncoding;
        use lofty::id3::v2::{
            ExtendedTextFrame, FrameId, Id3v2Tag, PrivateFrame, TextInformationFrame,
        };
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

    /// One `ItemKey` routes differently on ID3v2 and Vorbis, and a missing read
    /// mapping would leave every sort field looking permanently dirty.
    #[test]
    fn sort_names_round_trip_on_both_carriers() {
        let dir = scratch("sort-names");
        let fields = [
            (Field::TitleSort, "Lemon"),
            (Field::ArtistSort, "Yonezu, Kenshi"),
            (Field::AlbumArtistSort, "Yonezu, Kenshi"),
            (Field::AlbumSort, "Bootleg"),
        ];
        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            let changes: Vec<Change> = fields
                .iter()
                .map(|(field, value)| set(field.clone(), value))
                .collect();
            commit(&path, &changes).unwrap();
            let read_back = read(&path).unwrap();
            for (field, value) in &fields {
                assert_eq!(
                    value_of(&read_back, field).as_deref(),
                    Some(*value),
                    "{field:?} on {}",
                    path.display()
                );
            }

            let cleared: Vec<Change> = fields
                .iter()
                .map(|(field, _)| clear(field.clone()))
                .collect();
            commit(&path, &cleared).unwrap();
            let read_back = read(&path).unwrap();
            for (field, _) in &fields {
                assert_eq!(
                    value_of(&read_back, field),
                    None,
                    "{field:?} must clear on {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn m4a_fields_round_trip_over_untouched_audio() {
        let dir = scratch("m4a-round-trip");
        let path = m4a_file(&dir, "track.m4a");
        let fields = [
            (Field::Title, "Lemon"),
            (Field::Artist, "米津玄師"),
            (Field::Album, "Bootleg"),
            (Field::AlbumArtist, "米津玄師"),
            (Field::Genre, "J-Pop"),
            (Field::Year, "2018"),
            (Field::TrackNo, "3"),
            (Field::DiscNo, "1"),
            (Field::Comment, "kept"),
            (Field::Composer, "Kenshi Yonezu"),
            (Field::TitleSort, "Lemon"),
            (Field::ArtistSort, "Yonezu, Kenshi"),
            (Field::AlbumArtistSort, "Yonezu, Kenshi"),
            (Field::AlbumSort, "Bootleg"),
            (Field::Lyrics, "夢ならばどれほどよかったでしょう"),
            (Field::Custom("ROX_TEST".into()), "kept"),
        ];

        let changes: Vec<Change> = fields
            .iter()
            .map(|(field, value)| set(field.clone(), value))
            .collect();
        commit(&path, &changes).unwrap();
        let read_back = read(&path).unwrap();
        for (field, value) in &fields {
            assert_eq!(
                value_of(&read_back, field).as_deref(),
                Some(*value),
                "{field:?} must read back off the atom it was written to"
            );
        }
        assert!(
            fs::read(&path).unwrap().ends_with(&m4a_audio()),
            "the mdat moved, but its bytes are the same bytes"
        );
        assert_eq!(atoms_under(&path, "©lyr").len(), 1);

        let cleared: Vec<Change> = fields
            .iter()
            .map(|(field, _)| clear(field.clone()))
            .collect();
        commit(&path, &cleared).unwrap();
        let read_back = read(&path).unwrap();
        for (field, _) in &fields {
            assert_eq!(value_of(&read_back, field), None, "{field:?} must clear");
        }
    }

    /// Read and write have to agree on the key encoding, or the second edit
    /// writes a twin beside the atom it meant to change.
    #[test]
    fn m4a_custom_edits_change_one_atom_rather_than_growing_a_twin() {
        let dir = scratch("m4a-custom");
        let path = m4a_file(&dir, "track.m4a");
        let bare = Field::Custom("ROX_TEST".into());
        let owned = Field::Custom("com.rox.test:NOTE".into());

        commit(
            &path,
            &[set(bare.clone(), "first"), set(owned.clone(), "a")],
        )
        .unwrap();
        commit(
            &path,
            &[set(bare.clone(), "second"), set(owned.clone(), "b")],
        )
        .unwrap();

        assert_eq!(atoms_under(&path, "ROX_TEST"), vec!["second".to_string()]);
        assert_eq!(
            atoms_under(&path, "com.rox.test:NOTE"),
            vec!["b".to_string()]
        );
        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &bare).as_deref(), Some("second"));
        assert_eq!(value_of(&fields, &owned).as_deref(), Some("b"));

        commit(&path, &[clear(bare.clone()), clear(owned.clone())]).unwrap();
        assert!(atoms_under(&path, "ROX_TEST").is_empty());
        assert!(atoms_under(&path, "com.rox.test:NOTE").is_empty());
    }

    #[test]
    fn m4a_unknown_tags_read_and_edit_under_one_key() {
        let dir = scratch("m4a-unknown");
        let path = m4a_file(&dir, "track.m4a");
        let freeform = |name: &str, value: &str| {
            Atom::new(
                AtomIdent::Freeform {
                    mean: Cow::Borrowed(ITUNES_MEAN),
                    name: Cow::Owned(name.to_string()),
                },
                AtomData::UTF8(value.to_string()),
            )
        };
        let mut tag = Ilst::default();
        tag.insert(freeform("ISRC", "JPX000000001"));
        tag.insert(Atom::new(
            AtomIdent::Fourcc(*b"\xa9st3"),
            AtomData::UTF8("Sound engineer".to_string()),
        ));
        tag.insert(freeform(rating::FMPS_KEY, "0.8"));
        tag.insert(freeform("replaygain_track_gain", "-7.35 dB"));
        tag.insert(freeform(&embed_tag::key("test-model"), "v1;dim=2;f16;AAAA"));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "ISRC").as_deref(), Some("JPX000000001"));
        assert_eq!(text_of(&rows, "©st3").as_deref(), Some("Sound engineer"));
        for key in [
            rating::FMPS_KEY,
            "replaygain_track_gain",
            &embed_tag::key("test-model"),
        ] {
            assert!(
                unknown_of(&rows, key).is_none(),
                "{key} must stay out of the unknown list"
            );
        }

        commit(
            &path,
            &[
                set(Field::Unknown("ISRC".into()), "JPX000000002"),
                set(Field::Unknown("©st3".into()), "Mastering engineer"),
            ],
        )
        .unwrap();
        let rows = read_unknown(&path).unwrap();
        assert_eq!(text_of(&rows, "ISRC").as_deref(), Some("JPX000000002"));
        assert_eq!(
            text_of(&rows, "©st3").as_deref(),
            Some("Mastering engineer")
        );
        assert_eq!(atoms_under(&path, "ISRC").len(), 1);
        assert_eq!(atoms_under(&path, "©st3").len(), 1);

        commit(&path, &[clear(Field::Unknown("©st3".into()))]).unwrap();
        assert!(atoms_under(&path, "©st3").is_empty());
    }

    #[test]
    fn m4a_rating_round_trips_through_fmps_alone() {
        let dir = scratch("m4a-rating");
        let path = m4a_file(&dir, "track.m4a");

        commit(&path, &[set(Field::Rating, "7.5")]).unwrap();
        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Rating).as_deref(), Some("7.5"));
        assert_eq!(rating::read_path(&path), Some(75));
        assert_eq!(
            atoms_under(&path, rating::FMPS_KEY),
            vec!["0.75".to_string()]
        );
        assert_eq!(
            value_of(&fields, &Field::Custom(rating::FMPS_KEY.into())),
            None
        );

        commit(&path, &[clear(Field::Rating)]).unwrap();
        assert_eq!(rating::read_path(&path), None);
        assert!(atoms_under(&path, rating::FMPS_KEY).is_empty());
    }

    #[test]
    fn m4a_rating_edits_a_foreign_mean_fmps_atom_in_place() {
        let dir = scratch("m4a-rating-mean");
        let path = m4a_file(&dir, "track.m4a");
        let mut tag = Ilst::default();
        tag.insert(Atom::new(
            AtomIdent::Freeform {
                mean: Cow::Borrowed("org.example.tagger"),
                name: Cow::Borrowed(rating::FMPS_KEY),
            },
            AtomData::UTF8("0.60".to_string()),
        ));
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let fields = read(&path).unwrap();
        assert_eq!(value_of(&fields, &Field::Rating).as_deref(), Some("6"));
        assert!(
            !fields.iter().any(|(f, _)| matches!(f, Field::Custom(_))),
            "the rating atom must not surface as a custom row: {fields:?}"
        );
        assert!(read_unknown(&path).unwrap().is_empty());

        commit(&path, &[set(Field::Rating, "7.5")]).unwrap();
        assert_eq!(rating::read_path(&path), Some(75));
        assert_eq!(fmps_atoms(&path), vec!["0.75".to_string()]);
    }

    fn fmps_atoms(path: &Path) -> Vec<String> {
        parse_mp4(path)
            .unwrap()
            .ilst()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|atom| is_fmps_atom(atom.ident()))
            .flat_map(|atom| atom.into_data())
            .filter_map(|data| atom_text(&data))
            .collect()
    }

    #[test]
    fn m4a_cover_round_trips() {
        let dir = scratch("m4a-cover");
        let path = m4a_file(&dir, "track.m4a");

        commit_with(&path, &[], &[set_pic(PicKind::Front, jpeg(0x11))]).unwrap();
        let pictures = read_pictures(&path).unwrap();
        assert_eq!(pictures.len(), 1);
        assert_eq!(pictures[0].0, PicKind::Front);
        assert_eq!(pictures[0].1, jpeg(0x11));

        commit_with(
            &path,
            &[],
            &[PicChange {
                kind: PicKind::Front,
                data: None,
            }],
        )
        .unwrap();
        assert!(read_pictures(&path).unwrap().is_empty());
        assert!(fs::read(&path).unwrap().ends_with(&m4a_audio()));
    }

    /// The spans are recomputed on the clone: fixed offsets or a whole-file hash
    /// would both fail this commit.
    #[test]
    fn m4a_audio_hashes_the_same_after_the_tag_moves_it() {
        let dir = scratch("m4a-span");
        let path = m4a_file(&dir, "track.m4a");
        let before = crate::mp4::stream_spans(&path).unwrap();

        commit(&path, &[set(Field::Comment, &"padding ".repeat(512))]).unwrap();

        let after = crate::mp4::stream_spans(&path).unwrap();
        assert_eq!(after.len(), 1);
        assert!(
            after[0].start > before[0].start + 4000,
            "the tag has to actually move the audio: {} to {}",
            before[0].start,
            after[0].start
        );
        assert_eq!(after[0].end - after[0].start, m4a_audio().len() as u64);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[after[0].start as usize..], &m4a_audio()[..]);
    }

    #[test]
    fn fragmented_m4a_with_relative_offsets_commits() {
        let dir = scratch("m4a-fragmented-relative");
        let path = dir.join("track.m4a");
        fs::write(&path, m4a_fragmented_bytes(0x02_0000)).unwrap();
        let before = crate::mp4::stream_spans(&path).unwrap();
        assert_eq!(before.len(), 2);

        assert!(supported(&path));
        commit(
            &path,
            &[
                set(Field::Title, "Yesterwynde"),
                set(Field::Comment, &"padding ".repeat(512)),
            ],
        )
        .unwrap();

        let fields = read(&path).unwrap();
        assert!(fields.contains(&(Field::Title, "Yesterwynde".into())));
        let after = crate::mp4::stream_spans(&path).unwrap();
        assert_eq!(after.len(), 2);
        assert!(
            after[0].start > before[0].start + 4000,
            "the tag has to actually move the fragment: {} to {}",
            before[0].start,
            after[0].start
        );
        let bytes = fs::read(&path).unwrap();
        let moof = m4a_moof(0x02_0000);
        assert_eq!(
            &bytes[after[0].start as usize..after[0].end as usize],
            &moof[8..]
        );
        assert!(bytes.ends_with(&m4a_audio()));
    }

    /// A fragment placed by absolute position is refused with its own reason,
    /// since the editor shows it per file.
    #[test]
    fn fragmented_m4a_with_absolute_offsets_is_refused() {
        let dir = scratch("m4a-fragmented-absolute");
        let offset = dir.join("offset.m4a");
        fs::write(&offset, m4a_fragmented_bytes(0x02_0001)).unwrap();
        let indexed = dir.join("indexed.m4a");
        let mut bytes = m4a_fragmented_bytes(0x02_0000);
        bytes.extend(m4a_atom(b"sidx", &[0u8; 32]));
        fs::write(&indexed, &bytes).unwrap();

        for path in [&offset, &indexed] {
            assert!(!supported(path), "{}", path.display());
            let error = commit(path, &[set(Field::Title, "Nope")]).unwrap_err();
            assert!(error.contains("fragmented"), "{error}");
            assert!(read(path).is_err());
        }
        assert!(fs::read(&offset).unwrap().ends_with(&m4a_audio()));
        assert_eq!(fs::read(&indexed).unwrap(), bytes);
    }

    /// lofty maps ReplayGain to item keys, so it rides the split/merge; dropping it
    /// would silently unlevel a track.
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
            commit(&path, &[set(Field::Title, "Levelled")]).unwrap();

            let rg = crate::scanner::read_one(&path).unwrap().replay_gain;
            assert_eq!(rg.track_db, Some(-7.35), "{}", path.display());
            assert_eq!(rg.track_peak, Some(0.987654), "{}", path.display());
        }
    }

    /// Database thrown away, vector recovered from the file, on both formats.
    #[test]
    fn a_vector_written_into_a_file_outlives_its_database_row() {
        use crate::embeddings;

        let dir = scratch("embedding-round-trip");
        let vec: Vec<f32> = (0..64)
            .map(|i| (i as f32 - 32.0) * 0.37 + (i as f32) * (i as f32) * 0.02)
            .collect();

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::init_schema(&conn).unwrap();

        for path in [mp3_file(&dir, "track.mp3"), flac_file(&dir, "track.flac")] {
            let name = path.display().to_string();
            commit_embedding(&path, "builtin-v1", &vec).unwrap();

            let fields = read(&path).unwrap();
            assert!(
                !fields
                    .iter()
                    .any(|(f, _)| matches!(f, Field::Custom(k) if embed_tag::is_key(k))),
                "the vector must stay out of the field list, {name}"
            );

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

            assert!(embed_tag::read(&path, "panns-cnn10", vec.len()).is_none());
            assert!(embed_tag::read(&path, "builtin-v1", vec.len() + 1).is_none());
        }
    }

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

    /// Then a track-only re-measure clears the other three.
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

            // The strings another player reads, not lofty's own round trip.
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

            let rg = replaygain::read(&tag);
            assert_eq!(rg.track_db, Some(-6.5), "{file}");
            assert_eq!(rg.track_peak, Some(0.998762), "{file}");
            assert_eq!(rg.album_db, Some(-8.1), "{file}");
            assert_eq!(rg.album_peak, Some(1.023), "{file}");

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

    /// Plenty of taggers write lowercase TXXX descriptions. The generic key's
    /// case-insensitive match makes a set replace them.
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

    /// The lofty 0.24 carve-out: a bare Vorbis RATING survives an unrelated
    /// commit.
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

    /// A zero stuffed after every `ff` before a zero or sync-shaped byte, the same
    /// recipe as the art module's test.
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

    /// A TDRC lofty can't parse ("06-08") fails the whole read at the default
    /// parsing mode. Relaxed parsing drops the one frame.
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

    /// The Bandcamp shape: a text commit has to carry the mangled cover through
    /// byte-identical.
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

    /// Zeros between the tag end and the first frame, deeper than lofty's write
    /// probe searches.
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

    /// One surplus byte on a UTF-16 frame blanks the whole tag through lofty.
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

    /// The magic the art sniffer keys on, so the mime rescues to image/jpeg.
    fn jpeg(marker: u8) -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, marker, 0x2A, 0xFF, 0xD9]
    }

    fn set_pic(kind: PicKind, bytes: Vec<u8>) -> PicChange {
        PicChange {
            kind,
            data: Some((bytes, "image/jpeg".into())),
        }
    }

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

    /// An ID3v2.3 APIC typed `Other` (Windows Media Player's shape). Replacing the
    /// front must consolidate onto one typed cover.
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

        let pics = read_pictures(&path).unwrap();
        assert_eq!(pics.len(), 1);
        assert_eq!(pics[0].0, PicKind::Front);
        assert_eq!(pics[0].1, image);

        let new = jpeg(0x66);
        commit_with(&path, &[], &[set_pic(PicKind::Front, new.clone())]).unwrap();
        let pics = read_pictures(&path).unwrap();
        assert_eq!(pics.len(), 1, "the untyped cover must not orphan");
        assert_eq!(pics[0].1, new);
        assert_eq!(crate::art::cover_art(&path).unwrap().0, new);
    }
}
