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
use std::io::{Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType};
use lofty::flac::FlacFile;
use lofty::id3::v2::Frame;
use lofty::mpeg::MpegFile;
use lofty::ogg::OggPictureStorage;
use lofty::picture::{MimeType, Picture, PictureInformation, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag};

use crate::art;
use crate::rating;

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
    Custom(String),
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
        [PicKind::Front, PicKind::Back, PicKind::Media, PicKind::Artist]
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
        // The rating never writes as plain text; `apply_rating` puts its
        // popularimeter form on the generic tag itself.
        Field::Rating | Field::Custom(_) => return None,
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
                    out.push((Field::Custom(f.description.to_string()), f.content.to_string()));
                }
            }
        }
        FileType::Flac => {
            let tag = parse_flac(path)?.vorbis_comments().cloned().unwrap_or_default();
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

/// The named fields out of a split-off generic tag, in item order.
fn named_fields(generic: Tag, out: &mut Vec<(Field, String)>) {
    for item in generic.items() {
        let ItemValue::Text(text) = item.value() else {
            continue;
        };
        if let Some(field) = field_of(item.key()) {
            out.push((field, text.clone()));
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
            if let Some((data, mime)) = art::unsync_apic(path) {
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
    let result = catch_unwind(AssertUnwindSafe(|| commit_inner(path, &tmp, changes, pictures)))
        .unwrap_or_else(|_| Err(format!("tag parser panicked on {}", path.display())));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
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
        art::unsync_apic(path)
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
            // Read through the sanitiser so a tag lofty would de-unsync
            // twice parses clean; the write below zeroes the header flag,
            // so the saved clone no longer carries the shape at all.
            let mut source = crate::tag_source::open(tmp).map_err(|e| format!("open: {e}"))?;
            let mut mpeg = MpegFile::read_from(&mut source, parse_opts())
                .map_err(|e| format!("parse: {e}"))?;
            let mut tag = mpeg.id3v2().cloned().unwrap_or_default();
            for change in changes {
                if let Field::Custom(key) = &change.field {
                    match &change.value {
                        Some(v) => drop(tag.insert_user_text(key.clone(), v.clone())),
                        None => drop(tag.remove_user_text(key)),
                    }
                }
            }
            let (remainder, mut generic) = tag.split_tag();
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
            for change in changes {
                if let Field::Custom(key) = &change.field {
                    tag.remove(key).for_each(drop);
                    if let Some(v) = &change.value {
                        tag.push(key.clone(), v.clone());
                    }
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
/// the key, a clear drops them all.
fn apply_named(generic: &mut Tag, changes: &[Change]) {
    for change in changes {
        let Some(key) = item_key(&change.field) else {
            continue;
        };
        match &change.value {
            Some(v) => drop(generic.insert_text(key, v.clone())),
            None => generic.remove_key(key),
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
            let tag = parse_flac(tmp)?.vorbis_comments().cloned().unwrap_or_default();
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
        let read_back = match &change.field {
            Field::Custom(key) => customs
                .iter()
                .find(|(k, _)| k == key)
                .and_then(|(_, v)| v.clone()),
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
fn tmp_path(path: &Path) -> PathBuf {
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
    let len = file
        .metadata()
        .map_err(|e| format!("stat: {e}"))?
        .len();
    match kind {
        FileType::Mpeg => {
            let mut start = 0u64;
            let mut header = [0u8; 10];
            if file.read_exact(&mut header).is_ok() && &header[..3] == b"ID3" {
                let size = art::synchsafe(&header[6..10])
                    .ok_or("malformed ID3v2 size")? as u64;
                let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
                start = 10 + size + footer;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
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

    fn mp3_file(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, mpeg_audio()).unwrap();
        path
    }

    /// A bare FLAC container: magic, one last-flagged STREAMINFO claiming
    /// 44.1 kHz stereo 16-bit, then patterned bytes standing in for the
    /// frames.
    fn flac_file(dir: &Path, name: &str) -> PathBuf {
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
                !fields
                    .iter()
                    .any(|(f, _)| matches!(f, Field::Custom(k) if k.eq_ignore_ascii_case("FMPS_Rating"))),
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
            crate::tag_source::needs_unsync_repair(&path),
            "the shape flags before repair"
        );
        commit(&path, &[]).unwrap();
        assert!(
            !crate::tag_source::needs_unsync_repair(&path),
            "the rewrite clears the shape"
        );
        let (cover, mime) = crate::art::cover_art(&path).expect("the cover survives the repair");
        assert_eq!(cover, image);
        assert_eq!(mime, "image/jpeg");
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
                &[set_pic(PicKind::Back, back.clone()), set_pic(PicKind::Front, front2.clone())],
            )
            .unwrap();
            let pics = read_pictures(&path).unwrap();
            assert_eq!(pics.len(), 2);
            let of = |kind| pics.iter().find(|(k, _, _)| *k == kind).map(|(_, d, _)| d.clone());
            assert_eq!(of(PicKind::Front).as_deref(), Some(front2.as_slice()));
            assert_eq!(of(PicKind::Back).as_deref(), Some(back.as_slice()));

            // The front comes off, the back stays.
            commit_with(
                &path,
                &[],
                &[PicChange { kind: PicKind::Front, data: None }],
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
