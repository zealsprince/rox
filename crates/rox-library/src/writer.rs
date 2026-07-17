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
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, ItemValue, Tag};

use crate::art;

/// A tag field the editor can address. The named set is what the library
/// projects plus the fields a tag editor is expected to carry; `Custom`
/// is a format-specific key, an ID3v2 TXXX description or a Vorbis
/// comment key, written through the format tag so nothing re-maps it.
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
    Custom(String),
}

/// One field write; `None` clears the field.
#[derive(Clone, Debug)]
pub struct Change {
    pub field: Field,
    pub value: Option<String>,
}

/// One file's pending changes, the unit `commit_batch` takes.
pub struct Edit {
    pub path: PathBuf,
    pub changes: Vec<Change>,
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
        Field::Custom(_) => return None,
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
    let mut out = Vec::new();
    match file_type(path)? {
        FileType::Mpeg => {
            let tag = parse_mpeg(path)?.id3v2().cloned().unwrap_or_default();
            named_fields(tag.clone().split_tag().1, &mut out);
            for frame in &tag {
                if let Frame::UserText(f) = frame {
                    out.push((Field::Custom(f.description.to_string()), f.content.to_string()));
                }
            }
        }
        FileType::Flac => {
            let tag = parse_flac(path)?.vorbis_comments().cloned().unwrap_or_default();
            named_fields(tag.clone().split_tag().1, &mut out);
            for (key, value) in tag.items() {
                if ItemKey::from_key(lofty::tag::TagType::VorbisComments, key).is_none() {
                    out.push((Field::Custom(key.to_string()), value.to_string()));
                }
            }
        }
        _ => unreachable!("file_type only passes writable formats"),
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

/// Commit changes to one file through the atomic layer: clone, write the
/// clone, verify it (every change reads back, pictures byte-identical,
/// the audio stream hash unchanged), rename it over the original. Any
/// failure, including a parser panic, unlinks the clone and leaves the
/// original byte-identical.
pub fn commit(path: &Path, changes: &[Change]) -> Result<(), String> {
    let tmp = tmp_path(path);
    let result = catch_unwind(AssertUnwindSafe(|| commit_inner(path, &tmp, changes)))
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
        .map(|edit| (edit.path.clone(), commit(&edit.path, &edit.changes)))
        .collect()
}

fn commit_inner(path: &Path, tmp: &Path, changes: &[Change]) -> Result<(), String> {
    let kind = file_type(path)?;
    // What must hold after the write: the audio stream untouched and the
    // pictures byte-identical, with the raw re-read standing in for the
    // picture lofty mangles.
    let audio_hash = hash_span(path, audio_span(path, kind)?)?;
    let rescue = if kind == FileType::Mpeg {
        art::unsync_apic(path)
    } else {
        None
    };
    let expected_pictures = match kind {
        FileType::Mpeg => expected_pictures(path, rescue.as_ref())?,
        // FLAC pictures live in their own metadata blocks, outside the
        // unsync hazard; lofty carries them through whole.
        _ => Vec::new(),
    };

    fs::copy(path, tmp).map_err(|e| format!("copy for write: {e}"))?;
    write_tags(tmp, kind, changes, rescue)?;

    verify_fields(tmp, kind, changes)?;
    if kind == FileType::Mpeg {
        verify_pictures(tmp, &expected_pictures)?;
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
) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tmp)
        .map_err(|e| format!("open for write: {e}"))?;
    match kind {
        FileType::Mpeg => {
            let mut mpeg = MpegFile::read_from(&mut file, parse_opts())
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
            if let Some((data, mime)) = rescue {
                set_front_picture(&mut generic, data, &mime);
            }
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
            let mut flac = FlacFile::read_from(&mut file, parse_opts())
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
            apply_named(&mut generic, changes);
            flac.set_vorbis_comments(remainder.merge_tag(generic));
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
/// with the rescued raw bytes standing in for the front cover it mangles.
fn expected_pictures(
    path: &Path,
    rescue: Option<&(Vec<u8>, String)>,
) -> Result<Vec<Vec<u8>>, String> {
    let tag = parse_mpeg(path)?.id3v2().cloned().unwrap_or_default();
    let generic = tag.split_tag().1;
    let mut datas: Vec<Vec<u8>> = generic
        .pictures()
        .iter()
        .map(|p| p.data().to_vec())
        .collect();
    if let Some((data, _)) = rescue {
        let ix = generic
            .pictures()
            .iter()
            .position(|p| p.pic_type() == PictureType::CoverFront)
            .unwrap_or(0);
        match datas.get_mut(ix) {
            Some(slot) => *slot = data.clone(),
            None => datas.push(data.clone()),
        }
    }
    Ok(datas)
}

/// The clone's pictures against the expected set, compared as byte
/// multisets: the write may reorder frames, it may not touch an image.
fn verify_pictures(tmp: &Path, expected: &[Vec<u8>]) -> Result<(), String> {
    let tag = parse_mpeg(tmp)?.id3v2().cloned().unwrap_or_default();
    let mut got: Vec<Vec<u8>> = tag
        .split_tag()
        .1
        .pictures()
        .iter()
        .map(|p| p.data().to_vec())
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
    ParseOptions::new().read_properties(false)
}

fn parse_mpeg(path: &Path) -> Result<MpegFile, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    MpegFile::read_from(&mut file, parse_opts()).map_err(|e| format!("parse: {e}"))
}

fn parse_flac(path: &Path) -> Result<FlacFile, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    FlacFile::read_from(&mut file, parse_opts()).map_err(|e| format!("parse: {e}"))
}

/// The clone's path: a sibling in the same directory, so the final rename
/// never crosses a filesystem, with an extension the scanner ignores.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".rox-write");
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
            },
            Edit {
                path: bad,
                changes: vec![set(Field::Title, "Nope")],
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
}
