//! The artist store: the biography panel's data layer. What Last.fm
//! knows about an artist - wiki text, stats, tags, similar names - a
//! deezer portrait, and the wide banner and fanart theaudiodb carries,
//! fetched once and kept as plain files under the data directory's
//! artists folder, so a bio reads offline and a restart never refetches.
//! One JSON per artist under a stable hash of the folded name (the lyrics
//! store's naming move) with the image bytes beside it; an entry
//! refreshes once it ages past [`TTL_SECS`], and a fetch that fails with
//! a copy on disk serves the copy rather than nothing. Blocking,
//! background executor only, like the providers it calls.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{Image, ImageFormat};
use serde::{Deserialize, Serialize};

use rox_core::settings::artists_dir;
use rox_net::providers::{self, lastfm::ArtistInfo};

/// How long a cached entry serves before a fetch refreshes it. Bios and
/// stats drift slowly; a month keeps the network out of the loop
/// without pinning a first draft forever. Misses age the same way - a
/// misspelled tag that gets fixed is a different name and a different
/// entry anyway.
const TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// The longest side of a portrait thumbnail, the artist wall's tile
/// source. The same 256 the cover thumbnails use: enough for a tile at
/// any density, small enough that a wall of a thousand faces decodes
/// without eating the machine.
const THUMB_SIZE: u32 = 256;

/// Portrait thumbnails are JPEG at the cover store's quality; a face at
/// this size gains nothing from lossless.
const THUMB_QUALITY: u8 = 85;

/// A decoded image with its width-over-height ratio, so a panel can size
/// a frame to it and letterbox instead of cropping.
pub type SizedImage = (Arc<Image>, f32);

/// One artist as the panel shows them: the info sheet and the three
/// images, decoded and shareable. Any image can be absent - a service
/// that carries nothing for the artist, or an offline first look. The
/// header images carry their aspect ratio; the background fills and
/// crops, so it needs none.
#[derive(Clone)]
pub struct Artist {
    pub info: ArtistInfo,
    /// The square deezer portrait, the header's fallback.
    pub portrait: Option<SizedImage>,
    /// The wide theaudiodb banner, the header's first choice.
    pub banner: Option<SizedImage>,
    /// The theaudiodb fanart, the dimmed background behind the text.
    pub background: Option<Arc<Image>>,
}

/// The cache file's shape: when the fetch landed and what it found.
/// None inside records Last.fm not knowing the name, so a miss doesn't
/// re-query on every panel open.
#[derive(Serialize, Deserialize)]
struct Entry {
    fetched: u64,
    info: Option<ArtistInfo>,
}

/// The cache files for a name: the JSON and the three image slots beside
/// it, keyed on the folded name so casing and punctuation drift in the
/// tags shares one entry.
struct Files {
    info: PathBuf,
    portrait: PathBuf,
    banner: PathBuf,
    background: PathBuf,
    /// The portrait downscaled for a wall of tiles, generated from the
    /// full one and kept beside it.
    thumb: PathBuf,
}

fn files_for(name: &str) -> Files {
    let folded = providers::normalize(name);
    // Punctuation-only names ("!!!", "+/-") fold to nothing and would all
    // collide into one file - key those on the raw trimmed name instead.
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
        thumb: slot("thumb"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The artist under a name, cache first: a fresh entry answers from
/// disk, a stale or missing one fetches and rewrites it, and with the
/// artist provider off the cache answers at any age, so the panel still
/// works offline. `force` refetches past the TTL, the panel's refresh.
/// Ok(None) is a clean miss: Last.fm doesn't know the name, or nothing
/// is cached to serve offline. Blocking, background executor only.
pub fn get(name: &str, force: bool) -> Result<Option<Artist>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let files = files_for(name);
    let cached: Option<Entry> = fs::read_to_string(&files.info)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let fresh = cached
        .as_ref()
        .is_some_and(|entry| now().saturating_sub(entry.fetched) < TTL_SECS);
    if !providers::artist_online() || (fresh && !force) {
        let info = cached.and_then(|entry| entry.info);
        // A fresh entry can still be missing images - one transient
        // download failure shouldn't pin an empty slot for the whole TTL.
        // fetch_images skips slots whose files already stand, so this only
        // touches the network for what's absent, and never offline.
        if providers::artist_online() {
            if let Some(info) = &info {
                fetch_images(&info.name, &files, false);
            }
        }
        return Ok(info.map(|info| assemble(info, &files)));
    }
    match providers::lastfm::artist_info(name) {
        Ok(info) => {
            let entry = Entry {
                fetched: now(),
                info,
            };
            let _ = fs::create_dir_all(artists_dir());
            if let Ok(text) = serde_json::to_string(&entry) {
                let _ = fs::write(&files.info, text);
            }
            if let Some(info) = &entry.info {
                // The images search under Last.fm's spelling of the name,
                // not the tag's, so the services agree on who is meant.
                fetch_images(&info.name, &files, force);
            }
            Ok(entry.info.map(|info| assemble(info, &files)))
        }
        // The network failing with a copy on disk serves the copy; its
        // age beats an empty panel.
        Err(e) => match cached.and_then(|entry| entry.info) {
            Some(info) => Ok(Some(assemble(info, &files))),
            None => Err(e),
        },
    }
}

/// The artist's face as a small square, the artist wall's tile: the
/// deezer portrait downscaled once and kept beside the full one, so a
/// grid of a thousand names costs a few hundred KB of disk instead of a
/// gigabyte of full-size decodes. Reuses whatever the biography panel
/// already pulled down, so a shown artist costs no network at all.
///
/// `Ok(None)` is a settled miss - deezer answered and carries no picture
/// under the name - and an empty marker file makes it stick, so the wall
/// asks once per artist ever rather than on every scroll past. A network
/// failure is an `Err` and leaves the slots alone, so a blip doesn't pin a
/// blank face. Blocking, background executor only.
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
        // An empty portrait slot is a settled miss the wall left behind
        // last time; carry it into the thumbnail slot so this path is one
        // read from now on.
        Ok(_) => {
            mark_miss(&files.thumb);
            return Ok(None);
        }
        Err(_) => {
            if !providers::artist_online() {
                return Ok(None);
            }
            let Some(url) = providers::deezer::artist_picture(name)? else {
                // Settle both slots: the full portrait's fetch would ask the
                // same question and get the same nothing.
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
    // A portrait that won't decode is as good as none, and re-trying the
    // decode every paint would burn the pool; settle it.
    let Some(small) = downscale(&full) else {
        mark_miss(&files.thumb);
        return Ok(None);
    };
    let _ = fs::create_dir_all(artists_dir());
    let _ = fs::write(&files.thumb, &small);
    Ok(Some(small))
}

/// Settle a slot as a known miss: an empty file, which every reader here
/// treats as "nothing to show" and which stops the next look from asking
/// the network again. [`decode`] filters empty bytes, so a marker never
/// reaches the biography panel as a broken image.
fn mark_miss(file: &Path) {
    let _ = fs::create_dir_all(artists_dir());
    let _ = fs::write(file, []);
}

/// A full portrait's bytes into a square JPEG thumbnail, the cover store's
/// encode. None when the bytes won't decode as an image.
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

/// Land the artist's images beside the info file, quietly: the bio is the
/// panel's substance and a missing picture never fails it. The deezer
/// portrait and the theaudiodb banner and fanart, each skipped when its
/// file already stands unless the refresh is forced, so a service that
/// answered once isn't asked again every TTL.
fn fetch_images(name: &str, files: &Files, force: bool) {
    download(files.portrait.as_path(), force, || {
        providers::deezer::artist_picture(name)
    });
    // One theaudiodb call carries both wide images, so it runs only when a
    // slot is missing, not once per slot.
    if force || !files.banner.exists() || !files.background.exists() {
        if let Ok(Some(art)) = providers::theaudiodb::artist_art(name) {
            download(files.banner.as_path(), force, || Ok(art.banner));
            download(files.background.as_path(), force, || Ok(art.fanart));
        }
    }
}

/// Fetch one image slot from a URL the resolver hands back, writing the
/// bytes to `file`. Skips a slot that already stands unless forced, and a
/// resolver that offers no URL leaves the slot as it is.
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

/// The sheet with its images read off disk, decoded for the renderer.
fn assemble(info: ArtistInfo, files: &Files) -> Artist {
    Artist {
        info,
        portrait: decode(&files.portrait),
        banner: decode(&files.banner),
        // The background fills and crops, so it drops the ratio the header
        // frames need.
        background: decode(&files.background).map(|(image, _)| image),
    }
}

/// One image slot off disk, decoded with its aspect ratio; None when the
/// slot is empty or gone. The ratio comes off the header alone, no full
/// decode, the cover panel's move.
fn decode(file: &Path) -> Option<SizedImage> {
    let bytes = fs::read(file).ok().filter(|bytes| !bytes.is_empty())?;
    let ratio = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .map_or(1.0, |(w, h)| w as f32 / h.max(1) as f32);
    Some((Arc::new(Image::from_bytes(sniff(&bytes), bytes)), ratio))
}

/// The image format off the bytes themselves: the services serve jpeg and
/// png today, but the sniff keeps a format change from painting garbage.
fn sniff(bytes: &[u8]) -> ImageFormat {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => ImageFormat::Png,
        Ok(image::ImageFormat::WebP) => ImageFormat::Webp,
        Ok(image::ImageFormat::Gif) => ImageFormat::Gif,
        _ => ImageFormat::Jpeg,
    }
}
