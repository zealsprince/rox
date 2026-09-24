//! The biography panel's artist store: Last.fm's info and top tracks, a
//! deezer portrait, and theaudiodb's record and images, kept as plain files
//! under the artists folder so a bio reads offline. One JSON per artist
//! under a hash of the folded name, images beside it. A failed fetch serves
//! the stale copy. Blocking.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::theaudiodb::ArtistProfile;
use rox_net::providers::{self, lastfm::ArtistInfo};

const TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// The fetch asks for the most any view shows, once per artist.
pub const TOP_TRACKS: usize = 10;

/// theaudiodb holds up to four fanarts; the first is the background.
const EXTRA_FANARTS: usize = 3;

/// The same 256 the cover thumbnails use.
const THUMB_SIZE: u32 = 256;

const THUMB_QUALITY: u8 = 85;

#[derive(Clone)]
pub struct SizedImage {
    pub image: Arc<Image>,
    pub ratio: f32,
    /// A tiny blurred copy the renderer's upscale turns into a soft wash
    /// behind a letterboxed header. gpui has no runtime blur.
    pub soft: Option<Arc<Image>>,
}

const SOFT_SIZE: u32 = 32;

const SOFT_SIGMA: f32 = 1.5;

#[derive(Clone)]
pub struct Artist {
    pub info: ArtistInfo,
    pub profile: ArtistProfile,
    pub portrait: Option<SizedImage>,
    pub banner: Option<SizedImage>,
    /// A slot whose picture is the banner's is left out, so the header's
    /// cycle never shows one picture twice.
    pub fanarts: Vec<SizedImage>,
    pub background: Option<Arc<Image>>,
}

impl Artist {
    pub fn images(&self) -> Vec<Arc<Image>> {
        let mut out: Vec<Arc<Image>> = Vec::new();
        let candidates = self
            .portrait
            .iter()
            .chain(self.banner.iter())
            .chain(self.fanarts.iter())
            .flat_map(|sized| std::iter::once(&sized.image).chain(sized.soft.iter()))
            .chain(self.background.iter());
        for image in candidates {
            if !out.iter().any(|seen| seen.id() == image.id()) {
                out.push(image.clone());
            }
        }
        out
    }
}

/// `info: None` records a miss, so it doesn't re-query on every open.
#[derive(Serialize, Deserialize)]
struct Entry {
    fetched: u64,
    /// An entry in another language is stale however young. Absent on older
    /// entries, which read as English.
    #[serde(default)]
    lang: String,
    info: Option<ArtistInfo>,
    /// Some(empty) is a settled miss; None is never asked, so an older entry
    /// fills in on the next look without waiting out the TTL.
    #[serde(default)]
    profile: Option<ArtistProfile>,
}

/// The primary subtag, so en-CA asks for English.
fn bio_lang() -> String {
    rox_i18n::locale()
        .split('-')
        .next()
        .unwrap_or("en")
        .to_string()
}

struct Files {
    info: PathBuf,
    portrait: PathBuf,
    banner: PathBuf,
    background: PathBuf,
    fanarts: Vec<PathBuf>,
    thumb: PathBuf,
}

/// Removes the whole folder; every write here recreates it. Blocking.
pub fn clear() {
    let _ = fs::remove_dir_all(artists_dir());
}

fn files_for(name: &str) -> Files {
    let folded = providers::normalize(name);
    // Punctuation-only names ("!!!") fold to nothing, so key those raw.
    let key = if folded.is_empty() {
        name.trim()
    } else {
        &folded
    };
    let hash = rox_library::hash::fnv1a(key.as_bytes());
    let dir = artists_dir();
    let slot = |ext: &str| dir.join(format!("{hash:016x}.{ext}"));
    Files {
        info: slot("json"),
        portrait: slot("img"),
        banner: slot("banner"),
        background: slot("bg"),
        fanarts: (2..2 + EXTRA_FANARTS)
            .map(|n| slot(&format!("bg{n}")))
            .collect(),
        thumb: slot("thumb"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// With the artist provider off, the cache serves at any age. `force`
/// refetches past the TTL. Whatever the entry lacks past the Last.fm info
/// fills in on any online look, so a transient failure never pins a gap
/// for the whole TTL. Blocking.
pub fn get(name: &str, force: bool) -> Result<Option<Artist>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let files = files_for(name);
    let cached: Option<Entry> = fs::read_to_string(&files.info)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let lang = bio_lang();
    let fresh = cached.as_ref().is_some_and(|entry| {
        let young = now().saturating_sub(entry.fetched) < TTL_SECS;
        let same_lang = entry.lang.as_str() == lang || (entry.lang.is_empty() && lang == "en");
        // An entry without the wiki's links predates them: stale however young.
        let has_links = entry.info.as_ref().is_none_or(|info| info.links.is_some());
        young && same_lang && has_links
    });
    let online = providers::artist_online();
    let mut entry = cached;
    let mut dirty = false;
    if online && (!fresh || force) {
        match providers::lastfm::artist_info(name, &lang) {
            Ok(info) => {
                entry = Some(Entry {
                    fetched: now(),
                    lang: lang.clone(),
                    info,
                    profile: None,
                });
                dirty = true;
            }
            Err(e) => {
                if entry
                    .as_ref()
                    .and_then(|entry| entry.info.as_ref())
                    .is_none()
                {
                    return Err(e);
                }
            }
        }
    }
    let Some(mut entry) = entry else {
        return Ok(None);
    };
    if online && let Some(info) = entry.info.as_mut() {
        dirty |= complete(info, &mut entry.profile, &files, force);
    }
    if dirty {
        let _ = fs::create_dir_all(artists_dir());
        if let Ok(text) = serde_json::to_string(&entry) {
            let _ = fs::write(&files.info, text);
        }
    }
    let profile = entry.profile.unwrap_or_default();
    Ok(entry.info.map(|info| assemble(info, profile, &files)))
}

/// Each part runs only when missing or forced, and a failure is left for
/// the next look: none of it fails the bio. True when the entry changed.
fn complete(
    info: &mut ArtistInfo,
    profile: &mut Option<ArtistProfile>,
    files: &Files,
    force: bool,
) -> bool {
    let mut dirty = false;
    // Under Last.fm's spelling of the name, so the services agree on the artist.
    let name = info.name.clone();
    if force || info.top_tracks.is_none() {
        match providers::lastfm::top_tracks(&name, TOP_TRACKS) {
            Ok(tracks) => {
                info.top_tracks = Some(tracks);
                dirty = true;
            }
            Err(e) => log::debug!("artists: {name}: top tracks: {e}"),
        }
    }
    if force || profile.is_none() {
        match providers::theaudiodb::artist_profile(&name) {
            Ok(found) => {
                *profile = Some(found.unwrap_or_default());
                dirty = true;
            }
            Err(e) => log::debug!("artists: {name}: theaudiodb: {e}"),
        }
    }
    download(files.portrait.as_path(), force, || {
        providers::deezer::artist_picture(&name)
    });
    if let Some(profile) = profile {
        download(files.banner.as_path(), force, || Ok(profile.banner.clone()));
        download(files.background.as_path(), force, || {
            Ok(profile.fanart.clone())
        });
        for (file, url) in files.fanarts.iter().zip(profile.fanarts.iter().skip(1)) {
            download(file.as_path(), force, || Ok(Some(url.clone())));
        }
    }
    dirty
}

/// The artist wall's tile: the portrait downscaled once and kept beside
/// the full one. `Ok(None)` is a settled miss, stuck with an empty marker so
/// the wall asks once per artist ever. A network failure is an `Err` and
/// leaves the slots alone. Blocking.
pub fn portrait_thumb(name: &str) -> Result<Option<Vec<u8>>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let files = files_for(name);
    if let Ok(bytes) = fs::read(&files.thumb) {
        return Ok((!bytes.is_empty()).then_some(bytes));
    }
    let full = match fs::read(&files.portrait) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => {
            mark_miss(&files.thumb);
            return Ok(None);
        }
        Err(_) => {
            if !providers::artist_online() {
                return Ok(None);
            }
            let Some(url) = providers::deezer::artist_picture(name)? else {
                // Settle both slots: the full fetch would get the same nothing.
                mark_miss(&files.portrait);
                mark_miss(&files.thumb);
                return Ok(None);
            };
            let bytes = providers::fetch_image(&url)?;
            let _ = fs::create_dir_all(artists_dir());
            let _ = fs::write(&files.portrait, &bytes);
            bytes
        }
    };
    // Settle an undecodable portrait, or every paint retries the decode.
    let Some(small) = downscale(&full) else {
        mark_miss(&files.thumb);
        return Ok(None);
    };
    let _ = fs::create_dir_all(artists_dir());
    let _ = fs::write(&files.thumb, &small);
    Ok(Some(small))
}

/// An empty file, which every reader here treats as nothing to show.
fn mark_miss(file: &Path) {
    let _ = fs::create_dir_all(artists_dir());
    let _ = fs::write(file, []);
}

fn downscale(bytes: &[u8]) -> Option<Vec<u8>> {
    let full = image::load_from_memory(bytes).ok()?;
    let small = full.thumbnail(THUMB_SIZE, THUMB_SIZE).into_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY)
        .encode(
            small.as_raw(),
            small.width(),
            small.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(out)
}

/// Skips a slot that already stands unless forced.
fn download(file: &Path, force: bool, resolve: impl FnOnce() -> Result<Option<String>, String>) {
    if file.exists() && !force {
        return;
    }
    let Ok(Some(url)) = resolve() else {
        return;
    };
    if let Ok(bytes) = providers::fetch_image(&url) {
        let _ = fs::write(file, bytes);
    }
}

fn assemble(info: ArtistInfo, profile: ArtistProfile, files: &Files) -> Artist {
    let banner = decode(&files.banner);
    let background = decode(&files.background);
    // Pair each slot with its URL so a picture the banner already holds
    // stays out. An older entry has no URLs and keeps every slot.
    let mut slots: Vec<(Option<&str>, Option<SizedImage>)> =
        vec![(profile.fanart.as_deref(), background.clone())];
    for (file, url) in files.fanarts.iter().zip(profile.fanarts.iter().skip(1)) {
        slots.push((Some(url.as_str()), decode(file)));
    }
    let mut fanarts = Vec::new();
    let mut seen: Vec<&str> = profile.banner.as_deref().into_iter().collect();
    for (url, image) in slots {
        if let Some(url) = url {
            if seen.contains(&url) {
                continue;
            }
            seen.push(url);
        }
        if let Some(image) = image {
            fanarts.push(image);
        }
    }
    Artist {
        info,
        profile,
        portrait: decode(&files.portrait),
        banner,
        fanarts,
        background: background.map(|sized| sized.image),
    }
}

/// The ratio comes off the header alone, without a full decode.
fn decode(file: &Path) -> Option<SizedImage> {
    let bytes = fs::read(file).ok().filter(|bytes| !bytes.is_empty())?;
    let ratio = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map_or(1.0, |(w, h)| w as f32 / h.max(1) as f32);
    let soft = soft_for(file, &bytes);
    Some(SizedImage {
        image: Arc::new(Image::from_bytes(sniff(&bytes), bytes)),
        ratio,
        soft,
    })
}

/// Read from the `.soft` file beside the slot, or made once from a full
/// decode and written there.
fn soft_for(file: &Path, bytes: &[u8]) -> Option<Arc<Image>> {
    let mut soft_file = file.as_os_str().to_owned();
    soft_file.push(".soft");
    let soft_file = PathBuf::from(soft_file);
    if let Ok(png) = fs::read(&soft_file)
        && !png.is_empty()
    {
        return Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
    }
    let full = image::load_from_memory(bytes).ok()?;
    let small = full.thumbnail(SOFT_SIZE, SOFT_SIZE).blur(SOFT_SIGMA);
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    let _ = fs::write(&soft_file, &png);
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)))
}

/// Sniffed from the bytes, so a service changing formats can't paint garbage.
fn sniff(bytes: &[u8]) -> ImageFormat {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => ImageFormat::Png,
        Ok(image::ImageFormat::WebP) => ImageFormat::Webp,
        Ok(image::ImageFormat::Gif) => ImageFormat::Gif,
        _ => ImageFormat::Jpeg,
    }
}
