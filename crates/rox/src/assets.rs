//! The app's asset source: our own icons layered over the gpui-component
//! bundle. The widget set ships no media icons (play, skip, volume and
//! friends), so those live in `crates/rox/assets` and resolve first, with
//! everything else falling through to the bundled set.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

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
    pub const DISC: &str = "icons/disc-3.svg";
    pub const MUSIC: &str = "icons/music.svg";
    pub const LIST_MUSIC: &str = "icons/list-music.svg";
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
    /// The settings sidebars' page icons; the last three resolve from
    /// the bundled widget set, no file of ours needed.
    pub const SLIDERS: &str = "icons/sliders-horizontal.svg";
    pub const RADIO: &str = "icons/radio.svg";
    pub const DATABASE: &str = "icons/database.svg";
    pub const CLOCK: &str = "icons/clock.svg";
    pub const IMAGE: &str = "icons/image.svg";
    pub const PALETTE: &str = "icons/palette.svg";
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
    pub const AUDIO_LINES: &str = "icons/audio-lines.svg";
    pub const AUDIO_WAVEFORM: &str = "icons/audio-waveform.svg";
}

/// Our embedded assets, checked before the bundled widget assets so a
/// same-named file here wins.
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
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
