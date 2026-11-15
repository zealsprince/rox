//! The fonts the live workspace carries, handed to gpui's text system so a
//! look that names a family it brings draws that family on any machine.
//!
//! gpui has no way to take a font back out, so a font stays loaded until rox
//! restarts. A workspace applied earlier in the session can leave its
//! families behind in the picker. A font is untrusted bytes going to the
//! platform's parser, so each one is capped in size before it's read, the
//! session is capped in count and bytes, and a font the text system refuses
//! is skipped (the contract's Security rules).

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Read as _;
use std::sync::{LazyLock, Mutex};

use gpui::App;
use rox_core::settings::{Asset, AssetKind, assets};

/// The largest of the 3,800 font files on the machine this was tuned on is
/// 23 MB (a full CJK face), so this leaves headroom for a real font and still
/// bounds what one file can make the parser chew on.
pub(crate) const MAX_FONT_BYTES: u64 = 32_000_000;

/// A family runs to four or so faces, so this covers several workspaces'
/// worth in one session.
pub(crate) const MAX_FONTS: usize = 32;

/// Every loaded font's bytes stay in memory for the rest of the session.
const MAX_SESSION_BYTES: u64 = 256_000_000;

static LOADED: LazyLock<Mutex<Loaded>> = LazyLock::new(Mutex::default);

/// What this session has handed to the text system, keyed on the table's
/// hash. A font is counted when it's tried, so one the text system refuses
/// isn't read again on every apply.
#[derive(Default)]
struct Loaded {
    hashes: HashSet<String>,
    bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Load,
    Seen,
    Full,
}

impl Loaded {
    fn admit(&mut self, asset: &Asset) -> Admit {
        if self.hashes.contains(&asset.sha256) {
            return Admit::Seen;
        }

        let full = self.hashes.len() >= MAX_FONTS
            || asset.bytes > MAX_FONT_BYTES
            || self.bytes + asset.bytes > MAX_SESSION_BYTES;

        // Marked either way, so a refused font logs once, not on every apply.
        self.hashes.insert(asset.sha256.clone());
        if full {
            return Admit::Full;
        }

        self.bytes += asset.bytes;

        Admit::Load
    }
}

/// Runs on the UI thread, ahead of the next paint, on purpose: gpui caches
/// a family it failed to find for the rest of the session, so a font that
/// lands after the look first draws with it never shows.
pub(crate) fn register(cx: &mut App) {
    let fonts: Vec<Asset> = assets::all()
        .into_iter()
        .filter(|asset| asset.kind == AssetKind::Font)
        .collect();
    if fonts.is_empty() {
        return;
    }

    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let mut added = false;

    for asset in fonts {
        match loaded.admit(&asset) {
            Admit::Seen => continue,
            Admit::Full => {
                log::warn!(
                    "workspace fonts: skipping {} ({} bytes), past the session's font limits",
                    asset.file,
                    asset.bytes
                );
                continue;
            }
            Admit::Load => {}
        }

        let bytes = match read_capped(&asset) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("workspace fonts: reading {}: {e}", asset.file);
                continue;
            }
        };

        // One font a call: Windows and macOS give up on the whole batch at
        // the first file they can't parse.
        match cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
            Ok(()) => added = true,
            Err(e) => log::warn!("workspace fonts: loading {}: {e}", asset.file),
        }
    }

    if added {
        rox_panel_kit::fonts_changed();
    }
}

/// The table's size is only what the bundle claims, so the read stops one
/// byte past the cap on its own.
fn read_capped(asset: &Asset) -> Result<Vec<u8>, String> {
    let path = assets::path(asset).ok_or_else(|| format!("{} isn't a flat name", asset.file))?;
    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;

    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FONT_BYTES {
        return Err(format!("over {MAX_FONT_BYTES} bytes"));
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(sha256: &str, bytes: u64) -> Asset {
        Asset {
            id: sha256.to_string(),
            file: format!("{sha256}.ttf"),
            kind: AssetKind::Font,
            sha256: sha256.to_string(),
            bytes,
        }
    }

    #[test]
    fn a_font_registers_once_per_hash() {
        let mut loaded = Loaded::default();

        assert_eq!(loaded.admit(&font("a", 10)), Admit::Load);
        assert_eq!(loaded.admit(&font("a", 10)), Admit::Seen);
        assert_eq!(loaded.admit(&font("b", 10)), Admit::Load);
        assert_eq!(loaded.bytes, 20);
    }

    #[test]
    fn a_font_past_the_limits_is_refused_once() {
        let mut loaded = Loaded::default();

        assert_eq!(loaded.admit(&font("big", MAX_FONT_BYTES + 1)), Admit::Full);
        assert_eq!(loaded.admit(&font("big", MAX_FONT_BYTES + 1)), Admit::Seen);
        assert_eq!(loaded.bytes, 0);

        for n in 1..MAX_FONTS {
            assert_eq!(loaded.admit(&font(&n.to_string(), 1)), Admit::Load);
        }
        assert_eq!(loaded.admit(&font("one-more", 1)), Admit::Full);
    }
}
