//! The app's asset source: our icons in `crates/rox/assets` resolve first,
//! everything else falls through to the gpui-component bundle.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

use crate::palette;

/// Lucide icons, the same family as the bundled set.
pub mod icons {
    pub const PLAY: &str = "icons/play.svg";
    pub const PAUSE: &str = "icons/pause.svg";
    pub const SKIP_BACK: &str = "icons/skip-back.svg";
    pub const SKIP_FORWARD: &str = "icons/skip-forward.svg";
    pub const REWIND: &str = "icons/rewind.svg";
    pub const FAST_FORWARD: &str = "icons/fast-forward.svg";
    /// The step commands. Rewind and fast forward belong to the press-and-hold scan.
    pub const STEP_BACK: &str = "icons/step-back.svg";
    pub const STEP_FORWARD: &str = "icons/step-forward.svg";
    /// Double triangles alone read as track changes; the circular arrow is a
    /// jump by seconds.
    pub const SEEK_BACK: &str = "icons/rotate-ccw.svg";
    pub const SEEK_FORWARD: &str = "icons/rotate-cw.svg";
    pub const REPEAT: &str = "icons/repeat.svg";
    pub const REPEAT_1: &str = "icons/repeat-1.svg";
    /// A-B repeat: the flag is A set and waiting for B.
    pub const FLAG: &str = "icons/flag.svg";
    pub const ITERATION_CCW: &str = "icons/iteration-ccw.svg";
    pub const STOP: &str = "icons/square.svg";
    pub const SHUFFLE: &str = "icons/shuffle.svg";
    pub const LIST_ORDERED: &str = "icons/list-ordered.svg";
    pub const FLIP_HORIZONTAL: &str = "icons/flip-horizontal.svg";
    pub const FLIP_VERTICAL: &str = "icons/flip-vertical.svg";
    /// Continuation (ADR 17). Not the radio icon, which means ordering by sound.
    pub const INFINITY: &str = "icons/infinity.svg";
    pub const BLEND: &str = "icons/blend.svg";
    pub const DICE: &str = "icons/dice-5.svg";
    pub const VOLUME_1: &str = "icons/volume-1.svg";
    pub const VOLUME_2: &str = "icons/volume-2.svg";
    pub const VOLUME_X: &str = "icons/volume-x.svg";
    pub const HEADPHONES: &str = "icons/headphones.svg";
    pub const A_LARGE_SMALL: &str = "icons/a-large-small.svg";
    pub const ALIGN_LEFT: &str = "icons/align-left.svg";
    pub const ALIGN_CENTER: &str = "icons/align-center.svg";
    pub const ALIGN_RIGHT: &str = "icons/align-right.svg";
    pub const ROWS_2: &str = "icons/rows-2.svg";
    pub const ROWS_3: &str = "icons/rows-3.svg";
    pub const REFRESH_CW: &str = "icons/refresh-cw.svg";
    pub const POWER: &str = "icons/power.svg";
    pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
    pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
    pub const CHEVRON_UP: &str = "icons/chevron-up.svg";
    pub const DISC: &str = "icons/disc-3.svg";
    pub const LOCATE: &str = "icons/locate-fixed.svg";
    pub const MUSIC: &str = "icons/music.svg";
    pub const MIC: &str = "icons/mic.svg";
    pub const LIST_MUSIC: &str = "icons/list-music.svg";
    pub const SEARCH: &str = "icons/search.svg";
    pub const GLOBE: &str = "icons/globe.svg";
    pub const FUNNEL: &str = "icons/funnel.svg";
    pub const FOLDER: &str = "icons/folder.svg";
    pub const FOLDER_PLUS: &str = "icons/folder-plus.svg";
    pub const FILE_TEXT: &str = "icons/file-text.svg";
    pub const DOWNLOAD: &str = "icons/download.svg";
    pub const UPLOAD: &str = "icons/upload.svg";
    pub const TRASH: &str = "icons/trash-2.svg";
    /// The stats window's clear, where a trash icon would read as deleting a row.
    pub const BROOM: &str = "icons/brush-cleaning.svg";
    pub const PENCIL: &str = "icons/pencil.svg";
    pub const BOOKMARK: &str = "icons/bookmark.svg";
    pub const STAR: &str = "icons/star.svg";
    pub const STAR_FILLED: &str = "icons/star-filled.svg";
    pub const HEART: &str = "icons/heart.svg";
    pub const HEART_FILLED: &str = "icons/heart-filled.svg";
    pub const SLIDERS: &str = "icons/sliders-horizontal.svg";
    pub const RADIO: &str = "icons/radio.svg";
    pub const DATABASE: &str = "icons/database.svg";
    pub const CLOCK: &str = "icons/clock.svg";
    /// The tasks window. A clock would read as any of the time surfaces.
    pub const LIST_CHECKS: &str = "icons/list-checks.svg";
    /// The armed sleep timer, for the same reason.
    pub const BED: &str = "icons/bed.svg";
    pub const CALENDAR: &str = "icons/calendar.svg";
    pub const TAG: &str = "icons/tag.svg";
    pub const IMAGE: &str = "icons/image.svg";
    /// Only the icon picker offers this one.
    pub const WAND_SPARKLES: &str = "icons/wand-sparkles.svg";
    pub const LINK: &str = "icons/link.svg";
    pub const PALETTE: &str = "icons/palette.svg";
    pub const CONTRAST: &str = "icons/contrast.svg";
    pub const LAYOUT_DASHBOARD: &str = "icons/layout-dashboard.svg";
    pub const EYE: &str = "icons/eye.svg";
    pub const EYE_OFF: &str = "icons/eye-off.svg";
    pub const CLOSE: &str = "icons/close.svg";
    /// Stretched to the square's extent so it sits level beside maximize.
    pub const WINDOW_CLOSE: &str = "icons/window-close.svg";
    pub const CHECK: &str = "icons/check.svg";
    pub const SETTINGS: &str = "icons/settings-2.svg";
    pub const COPY: &str = "icons/copy.svg";
    pub const EXTERNAL_LINK: &str = "icons/external-link.svg";
    pub const PLUS: &str = "icons/plus.svg";
    pub const CHART_PIE: &str = "icons/chart-pie.svg";
    pub const INFO: &str = "icons/info.svg";
    pub const ALERT: &str = "icons/triangle-alert.svg";
    pub const BUG: &str = "icons/bug.svg";
    pub const MESSAGES: &str = "icons/messages-square.svg";
    pub const HASH: &str = "icons/hash.svg";
    pub const LAYOUT_GRID: &str = "icons/layout-grid.svg";
    pub const GALLERY: &str = "icons/gallery-horizontal-end.svg";
    pub const MOVE_VERTICAL: &str = "icons/move-vertical.svg";
    pub const MOVE_HORIZONTAL: &str = "icons/move-horizontal.svg";
    pub const AUDIO_LINES: &str = "icons/audio-lines.svg";
    pub const AUDIO_WAVEFORM: &str = "icons/audio-waveform.svg";
    pub const GAUGE: &str = "icons/gauge.svg";
    pub const ACTIVITY: &str = "icons/activity.svg";
    pub const WAVES: &str = "icons/waves.svg";
    pub const MINIMIZE: &str = "icons/minimize-2.svg";
    pub const MAXIMIZE: &str = "icons/maximize-2.svg";
    /// Bracket-style so it can't be mistaken for the mini toggle's arrows.
    pub const FULLSCREEN_EXIT: &str = "icons/minimize.svg";
    pub const SQUARE_DASHED: &str = "icons/square-dashed.svg";
    /// Unfinished work, on both the Development page and the experimental
    /// panel group.
    pub const FLASK: &str = "icons/flask-conical.svg";
    pub const SQUARE_TERMINAL: &str = "icons/square-terminal.svg";
    pub const KEYBOARD: &str = "icons/keyboard.svg";
    pub const SUN: &str = "icons/sun.svg";
    pub const MOON: &str = "icons/moon.svg";
    pub const MINUS: &str = "icons/minus.svg";
    pub const APP_WINDOW: &str = "icons/app-window.svg";
    pub const MENU: &str = "icons/menu.svg";
    pub const USER: &str = "icons/user-round.svg";
    pub const MOVE: &str = "icons/move.svg";
    pub const COLUMNS_2: &str = "icons/columns-2.svg";
    pub const LAYERS: &str = "icons/layers-2.svg";
    pub const PANEL_BOTTOM: &str = "icons/panel-bottom.svg";
    pub const PANEL_TOP: &str = "icons/panel-top.svg";
    pub const PANEL_LEFT: &str = "icons/panel-left.svg";
    pub const PANEL_RIGHT: &str = "icons/panel-right.svg";
    pub const CHEVRON_LEFT: &str = "icons/chevron-left.svg";
    pub const LOCK: &str = "icons/lock.svg";
    pub const LOCK_OPEN: &str = "icons/lock-open.svg";
    pub const PIN: &str = "icons/pin.svg";
    pub const ARROW_UP: &str = "icons/arrow-up.svg";
    pub const ARROW_DOWN: &str = "icons/arrow-down.svg";
    pub const ARROW_LEFT: &str = "icons/arrow-left.svg";
    /// Nothing draws this yet; it keeps the four arrows complete.
    pub const ARROW_RIGHT: &str = "icons/arrow-right.svg";
    /// A single-path logo, so it paints in the text color.
    pub const LOGO: &str = "app/rox-music.svg";

    /// Every icon the app draws, the set an icon picker offers, in picker
    /// order: button glyphs first, window and panel furniture last. The logo
    /// is left out: it's the brand mark, not a button icon.
    pub const CATALOG: &[&str] = &[
        PLAY,
        PAUSE,
        SKIP_BACK,
        SKIP_FORWARD,
        STEP_BACK,
        STEP_FORWARD,
        REWIND,
        FAST_FORWARD,
        SEEK_BACK,
        SEEK_FORWARD,
        REPEAT,
        REPEAT_1,
        FLAG,
        ITERATION_CCW,
        STOP,
        SHUFFLE,
        LIST_ORDERED,
        FLIP_HORIZONTAL,
        FLIP_VERTICAL,
        INFINITY,
        BLEND,
        DICE,
        VOLUME_1,
        VOLUME_2,
        VOLUME_X,
        HEADPHONES,
        A_LARGE_SMALL,
        ALIGN_LEFT,
        ALIGN_CENTER,
        ALIGN_RIGHT,
        ROWS_2,
        ROWS_3,
        REFRESH_CW,
        POWER,
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
        BOOKMARK,
        STAR,
        STAR_FILLED,
        HEART,
        HEART_FILLED,
        SLIDERS,
        RADIO,
        DATABASE,
        CLOCK,
        LIST_CHECKS,
        BED,
        CALENDAR,
        TAG,
        IMAGE,
        WAND_SPARKLES,
        LINK,
        PALETTE,
        CONTRAST,
        LAYOUT_DASHBOARD,
        EYE,
        EYE_OFF,
        SETTINGS,
        COPY,
        CHART_PIE,
        INFO,
        ALERT,
        BUG,
        MESSAGES,
        HASH,
        LAYOUT_GRID,
        GALLERY,
        MOVE_HORIZONTAL,
        AUDIO_LINES,
        AUDIO_WAVEFORM,
        GAUGE,
        ACTIVITY,
        WAVES,
        MINIMIZE,
        MAXIMIZE,
        FULLSCREEN_EXIT,
        SQUARE_DASHED,
        FLASK,
        SQUARE_TERMINAL,
        KEYBOARD,
        SUN,
        MOON,
        APP_WINDOW,
        USER,
        LAYERS,
        LOCK,
        LOCK_OPEN,
        PIN,
        // The chrome, sunk to the bottom because a button rarely wants one.
        CHEVRON_UP,
        CHEVRON_DOWN,
        CHEVRON_LEFT,
        CHEVRON_RIGHT,
        ARROW_UP,
        ARROW_DOWN,
        ARROW_LEFT,
        ARROW_RIGHT,
        PLUS,
        MINUS,
        CLOSE,
        CHECK,
        MOVE,
        MOVE_VERTICAL,
        PANEL_TOP,
        PANEL_BOTTOM,
        PANEL_LEFT,
        PANEL_RIGHT,
        COLUMNS_2,
        MENU,
        EXTERNAL_LINK,
    ];
}

/// Checked before the bundled assets, so a same-named file here wins.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../rox/assets"]
#[include = "icons/**/*.svg"]
#[include = "app/rox-music.svg"]
#[include = "workspaces/**/*.json"]
#[include = "workspaces/**/*.png"]
#[include = "disc/*.png"]
pub struct Assets;

pub fn shipped_workspaces() -> Vec<(String, Cow<'static, [u8]>)> {
    shipped_json("workspaces/")
}

/// `workspaces/<stem>_Dark.png` or `_Light.png`, falling back to a plain
/// `<stem>.png` for both sides. Keyed by file stem, not bundle name.
pub fn workspace_preview(stem: &str, mode: palette::Mode) -> Option<SharedString> {
    let side = match mode {
        palette::Mode::Dark => "Dark",
        palette::Mode::Light => "Light",
    };
    [
        format!("workspaces/{stem}_{side}.png"),
        format!("workspaces/{stem}.png"),
    ]
    .into_iter()
    .find(|path| Assets::get(path).is_some())
    .map(Into::into)
}

/// Read straight from the IHDR header, so the quick-start tile's hover pan
/// sweeps the whole screenshot.
pub fn png_aspect(path: &str) -> Option<f32> {
    let bytes = Assets::get(path)?.data;
    // Signature (8 bytes) and the IHDR chunk header (8 more), then width
    // and height as big-endian u32s.
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    (w > 0 && h > 0).then(|| w as f32 / h as f32)
}

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

    #[test]
    fn every_catalog_icon_resolves() {
        for path in icons::CATALOG {
            let bytes = Assets.load(path).unwrap();
            assert!(
                bytes.is_some_and(|b| !b.is_empty()),
                "catalog icon {path} does not resolve"
            );
        }
    }
}
