//! The app's asset source: our own icons layered over the gpui-component
//! bundle. The widget set ships no media icons (play, skip, volume and
//! friends), so those live in `crates/rox/assets` and resolve first, with
//! everything else falling through to the bundled set.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::RwLock;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

/// The active icon pack's folder, or None for the built-in set. A pack is a
/// flat folder of SVGs named like the built-in icons (play.svg, heart.svg);
/// a file present there overrides that icon, a missing one falls through to
/// our own embedded set and then the bundled widget set. Set at startup from
/// settings and when the picker changes. Already-rendered icons keep their
/// tiles until the app restarts, since gpui's sprite atlas keys on the path
/// and only reads an icon's bytes on a cache miss.
static ACTIVE_PACK: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Point the resolver at an icon pack folder, or None for the built-in set.
/// [`crate::startup::icon_packs`] owns the name-to-folder mapping and calls this.
pub fn set_active_pack(dir: Option<PathBuf>) {
    *ACTIVE_PACK.write().unwrap() = dir;
}

/// The built-in bytes for an icon path, ours first then the bundled widget
/// set, ignoring any active pack. Seeds a new pack folder with the current
/// icons so an author starts from the real set, not a blank folder.
pub fn builtin_bytes(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(f) = Assets::get(path) {
        return Some(f.data);
    }
    gpui_component_assets::Assets.load(path).ok().flatten()
}

/// Icon paths for gpui's `svg` element. Lucide icons, the same family the
/// bundled widget icons come from, so the two sets match on screen.
pub mod icons {
    pub const PLAY: &str = "icons/play.svg";
    pub const PAUSE: &str = "icons/pause.svg";
    pub const SKIP_BACK: &str = "icons/skip-back.svg";
    pub const SKIP_FORWARD: &str = "icons/skip-forward.svg";
    pub const REWIND: &str = "icons/rewind.svg";
    pub const FAST_FORWARD: &str = "icons/fast-forward.svg";
    pub const REPEAT: &str = "icons/repeat.svg";
    pub const REPEAT_1: &str = "icons/repeat-1.svg";
    pub const STOP: &str = "icons/square.svg";
    pub const SHUFFLE: &str = "icons/shuffle.svg";
    pub const DICE: &str = "icons/dice-5.svg";
    pub const VOLUME_1: &str = "icons/volume-1.svg";
    pub const VOLUME_2: &str = "icons/volume-2.svg";
    pub const VOLUME_X: &str = "icons/volume-x.svg";
    pub const ALIGN_LEFT: &str = "icons/align-left.svg";
    pub const ALIGN_CENTER: &str = "icons/align-center.svg";
    pub const ALIGN_RIGHT: &str = "icons/align-right.svg";
    pub const ROWS_2: &str = "icons/rows-2.svg";
    pub const ROWS_3: &str = "icons/rows-3.svg";
    pub const REFRESH_CW: &str = "icons/refresh-cw.svg";
    pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
    pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
    pub const DISC: &str = "icons/disc-3.svg";
    pub const LOCATE: &str = "icons/locate-fixed.svg";
    pub const MUSIC: &str = "icons/music.svg";
    pub const MIC: &str = "icons/mic.svg";
    pub const LIST_MUSIC: &str = "icons/list-music.svg";
    /// These two resolve from the bundled widget set, no file of ours needed.
    pub const SEARCH: &str = "icons/search.svg";
    pub const GLOBE: &str = "icons/globe.svg";
    pub const FUNNEL: &str = "icons/funnel.svg";
    pub const FOLDER: &str = "icons/folder.svg";
    pub const FOLDER_PLUS: &str = "icons/folder-plus.svg";
    pub const FILE_TEXT: &str = "icons/file-text.svg";
    pub const DOWNLOAD: &str = "icons/download.svg";
    pub const UPLOAD: &str = "icons/upload.svg";
    pub const TRASH: &str = "icons/trash-2.svg";
    pub const PENCIL: &str = "icons/pencil.svg";
    /// The rating stars: the same outline both ways, the filled one
    /// carrying its fill so the pair reads set/unset at cell size.
    pub const STAR: &str = "icons/star.svg";
    pub const STAR_FILLED: &str = "icons/star-filled.svg";
    /// The favourites heart, outline and filled, the set/unset pair the
    /// same way the stars work.
    pub const HEART: &str = "icons/heart.svg";
    pub const HEART_FILLED: &str = "icons/heart-filled.svg";
    /// The settings sidebars' page icons; the last three resolve from
    /// the bundled widget set, no file of ours needed.
    pub const SLIDERS: &str = "icons/sliders-horizontal.svg";
    pub const RADIO: &str = "icons/radio.svg";
    pub const DATABASE: &str = "icons/database.svg";
    pub const CLOCK: &str = "icons/clock.svg";
    pub const IMAGE: &str = "icons/image.svg";
    pub const PALETTE: &str = "icons/palette.svg";
    pub const CONTRAST: &str = "icons/contrast.svg";
    pub const LAYOUT_DASHBOARD: &str = "icons/layout-dashboard.svg";
    pub const EYE: &str = "icons/eye.svg";
    /// These two resolve from the bundled widget set, no file of ours
    /// needed.
    pub const CLOSE: &str = "icons/close.svg";
    pub const CHECK: &str = "icons/check.svg";
    /// The panel menu's icons, all from the bundled widget set too.
    pub const SETTINGS: &str = "icons/settings-2.svg";
    pub const COPY: &str = "icons/copy.svg";
    pub const EXTERNAL_LINK: &str = "icons/external-link.svg";
    /// The menubar dropdown icons; the first three resolve from the
    /// bundled widget set.
    pub const PLUS: &str = "icons/plus.svg";
    pub const CHART_PIE: &str = "icons/chart-pie.svg";
    pub const INFO: &str = "icons/info.svg";
    pub const LAYOUT_GRID: &str = "icons/layout-grid.svg";
    pub const GALLERY: &str = "icons/gallery-horizontal-end.svg";
    pub const MOVE_VERTICAL: &str = "icons/move-vertical.svg";
    pub const MOVE_HORIZONTAL: &str = "icons/move-horizontal.svg";
    pub const AUDIO_LINES: &str = "icons/audio-lines.svg";
    pub const AUDIO_WAVEFORM: &str = "icons/audio-waveform.svg";
    /// The mini-player toggle: shrink into the mini layout, grow back to
    /// the primary.
    pub const MINIMIZE: &str = "icons/minimize-2.svg";
    pub const MAXIMIZE: &str = "icons/maximize-2.svg";
    /// The Window menu's Empty Window entry: a blank dock.
    pub const SQUARE_DASHED: &str = "icons/square-dashed.svg";
    /// The window controls panel's icon style, and the OS decorations
    /// menu entry.
    pub const MINUS: &str = "icons/minus.svg";
    pub const APP_WINDOW: &str = "icons/app-window.svg";
    /// The menu panel's button.
    pub const MENU: &str = "icons/menu.svg";
    /// The biography panel: an artist as a person.
    pub const USER: &str = "icons/user-round.svg";
    /// The drag anchor panel's grip.
    pub const MOVE: &str = "icons/move.svg";
    /// The composition panels: the group's split, the depth panel's
    /// stacked layers, and the slide panel's back arrow (forward is
    /// CHEVRON_RIGHT; left resolves from the bundled widget set).
    pub const COLUMNS_2: &str = "icons/columns-2.svg";
    pub const LAYERS: &str = "icons/layers-2.svg";
    pub const CHEVRON_LEFT: &str = "icons/chevron-left.svg";
    /// The layout tree's per-panel lock toggle, pinned and free.
    pub const LOCK: &str = "icons/lock.svg";
    pub const LOCK_OPEN: &str = "icons/lock-open.svg";
    /// The layout tree's reorder arrows and lift-out, from the bundled
    /// widget set.
    pub const ARROW_UP: &str = "icons/arrow-up.svg";
    pub const ARROW_DOWN: &str = "icons/arrow-down.svg";
    pub const ARROW_LEFT: &str = "icons/arrow-left.svg";
    /// The rox mark: a single-path logo, so it paints in the text color
    /// like any other svg element. Heads the empty launcher and the
    /// welcome window.
    pub const LOGO: &str = "app/rox-music.svg";

    /// Every icon the app draws, the surface an icon pack can override. A
    /// new pack is seeded with these, so an author edits the real files
    /// instead of guessing which names the app asks for. The logo is left
    /// out on purpose: it is the brand mark, not a themeable icon.
    pub const CATALOG: &[&str] = &[
        PLAY,
        PAUSE,
        SKIP_BACK,
        SKIP_FORWARD,
        REWIND,
        FAST_FORWARD,
        REPEAT,
        REPEAT_1,
        STOP,
        SHUFFLE,
        DICE,
        VOLUME_1,
        VOLUME_2,
        VOLUME_X,
        ALIGN_LEFT,
        ALIGN_CENTER,
        ALIGN_RIGHT,
        ROWS_2,
        ROWS_3,
        REFRESH_CW,
        CHEVRON_RIGHT,
        CHEVRON_DOWN,
        DISC,
        LOCATE,
        MUSIC,
        MIC,
        LIST_MUSIC,
        SEARCH,
        GLOBE,
        FUNNEL,
        FOLDER,
        FOLDER_PLUS,
        FILE_TEXT,
        DOWNLOAD,
        UPLOAD,
        TRASH,
        PENCIL,
        STAR,
        STAR_FILLED,
        HEART,
        HEART_FILLED,
        SLIDERS,
        RADIO,
        DATABASE,
        CLOCK,
        IMAGE,
        PALETTE,
        CONTRAST,
        LAYOUT_DASHBOARD,
        EYE,
        CLOSE,
        CHECK,
        SETTINGS,
        COPY,
        EXTERNAL_LINK,
        PLUS,
        CHART_PIE,
        INFO,
        LAYOUT_GRID,
        GALLERY,
        MOVE_VERTICAL,
        MOVE_HORIZONTAL,
        AUDIO_LINES,
        AUDIO_WAVEFORM,
        MINIMIZE,
        MAXIMIZE,
        SQUARE_DASHED,
        MINUS,
        APP_WINDOW,
        MENU,
        USER,
        MOVE,
        COLUMNS_2,
        LAYERS,
        CHEVRON_LEFT,
        LOCK,
        LOCK_OPEN,
        ARROW_UP,
        ARROW_DOWN,
        ARROW_LEFT,
    ];
}

/// Our embedded assets, checked before the bundled widget assets so a
/// same-named file here wins.
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
#[include = "app/rox-music.svg"]
#[include = "workspaces/**/*.json"]
pub struct Assets;

/// The shipped workspace bundles: every JSON file under `assets/workspaces`,
/// as `(file stem, raw bytes)`. The workspaces module names and parses them.
pub fn shipped_workspaces() -> Vec<(String, Cow<'static, [u8]>)> {
    shipped_json("workspaces/")
}

/// Every shipped `.json` under one asset folder, as `(file stem, raw bytes)`.
fn shipped_json(prefix: &str) -> Vec<(String, Cow<'static, [u8]>)> {
    Assets::iter()
        .filter_map(|path| {
            let rest = path.strip_prefix(prefix)?.strip_suffix(".json")?;
            let file = Assets::get(path.as_ref())?;
            Some((rest.to_string(), file.data))
        })
        .collect()
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        // An active pack overrides same-named files under icons/. It is a
        // flat folder, so map icons/play.svg to <pack>/play.svg. A missing
        // file just falls through to the built-in and bundled sets below.
        if let Some(name) = path.strip_prefix("icons/") {
            if let Some(dir) = ACTIVE_PACK.read().unwrap().clone() {
                if let Ok(bytes) = std::fs::read(dir.join(name)) {
                    return Ok(Some(Cow::Owned(bytes)));
                }
            }
        }
        if let Some(f) = Self::get(path) {
            return Ok(Some(f.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
        entries.extend(gpui_component_assets::Assets.list(path)?);
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An active pack overrides a same-named icon and falls through for one
    /// it doesn't carry, the whole point of the layered resolver.
    #[test]
    fn active_pack_overrides_then_falls_through() {
        let dir = std::env::temp_dir().join("rox-pack-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("play.svg"), b"<svg id=\"packed\"/>").unwrap();

        set_active_pack(Some(dir.clone()));
        // The pack carries play.svg, so it wins.
        let play = Assets.load("icons/play.svg").unwrap().unwrap();
        assert_eq!(play.as_ref(), b"<svg id=\"packed\"/>");
        // It carries no pause.svg, so that falls through to the built-in.
        let pause = Assets.load("icons/pause.svg").unwrap().unwrap();
        assert_ne!(pause.as_ref(), b"<svg id=\"packed\"/>");
        assert!(!pause.is_empty());

        // Back to the built-in set: play.svg no longer comes from the pack.
        set_active_pack(None);
        let play = Assets.load("icons/play.svg").unwrap().unwrap();
        assert_ne!(play.as_ref(), b"<svg id=\"packed\"/>");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A new pack is seeded from the catalog, so every entry has to resolve
    /// to real bytes, ours or bundled. If one doesn't, a created pack would
    /// silently miss that icon and the app would fall back mid-set.
    #[test]
    fn every_catalog_icon_resolves() {
        for path in icons::CATALOG {
            assert!(
                builtin_bytes(path).is_some(),
                "catalog icon {path} has no built-in bytes"
            );
            assert!(
                path.strip_prefix("icons/").is_some(),
                "catalog icon {path} is not under icons/, so a flat pack can't hold it"
            );
        }
    }
}
