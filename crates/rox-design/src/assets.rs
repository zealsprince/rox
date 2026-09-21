//! The app's asset source: our own icons layered over the gpui-component
//! bundle. The widget set ships no media icons (play, skip, volume and
//! friends), so those are in `crates/rox/assets` and resolve first, with
//! everything else falling through to the bundled set.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

use crate::palette;

/// Icon paths for gpui's `svg` element. Lucide icons, the same family the
/// bundled widget icons come from, so the two sets match on screen.
pub mod icons {
    pub const PLAY: &str = "icons/play.svg";
    pub const PAUSE: &str = "icons/pause.svg";
    pub const SKIP_BACK: &str = "icons/skip-back.svg";
    pub const SKIP_FORWARD: &str = "icons/skip-forward.svg";
    pub const REWIND: &str = "icons/rewind.svg";
    pub const FAST_FORWARD: &str = "icons/fast-forward.svg";
    /// The step commands, one track's worth at a time. Rewind and fast
    /// forward are spoken for by the press-and-hold scan, and the bar
    /// against the triangle is the mark for a single step.
    pub const STEP_BACK: &str = "icons/step-back.svg";
    pub const STEP_FORWARD: &str = "icons/step-forward.svg";
    /// The timed nudges where no skip buttons are alongside to disambiguate:
    /// on their own, the double triangles read as track changes, and the
    /// circular arrow is the established mark for a jump by seconds.
    pub const SEEK_BACK: &str = "icons/rotate-ccw.svg";
    pub const SEEK_FORWARD: &str = "icons/rotate-cw.svg";
    pub const REPEAT: &str = "icons/repeat.svg";
    pub const REPEAT_1: &str = "icons/repeat-1.svg";
    /// A-B repeat: the flag is the a-point planted and waiting for b, the
    /// counter-clockwise iteration is the loop once both ends are set.
    pub const FLAG: &str = "icons/flag.svg";
    pub const ITERATION_CCW: &str = "icons/iteration-ccw.svg";
    pub const STOP: &str = "icons/square.svg";
    pub const SHUFFLE: &str = "icons/shuffle.svg";
    /// Shuffle off: the queue in the order it was written, which is what
    /// the numbered list draws.
    pub const LIST_ORDERED: &str = "icons/list-ordered.svg";
    /// The Milkdrop panel's mirror toggles, one per axis the frame can be
    /// flipped on.
    pub const FLIP_HORIZONTAL: &str = "icons/flip-horizontal.svg";
    pub const FLIP_VERTICAL: &str = "icons/flip-vertical.svg";
    /// Continuation (ADR 17): a queue that doesn't end. The lemniscate reads
    /// as "this keeps going" without borrowing the radio, which already means
    /// ordering by sound rather than never stopping.
    pub const INFINITY: &str = "icons/infinity.svg";
    /// Crossfade (ADR 19): two circles overlapping, which is the picture of
    /// one track lying over the next.
    pub const BLEND: &str = "icons/blend.svg";
    pub const DICE: &str = "icons/dice-5.svg";
    pub const VOLUME_1: &str = "icons/volume-1.svg";
    pub const VOLUME_2: &str = "icons/volume-2.svg";
    pub const VOLUME_X: &str = "icons/volume-x.svg";
    /// Exclusive output: rox holding the device on its own.
    pub const HEADPHONES: &str = "icons/headphones.svg";
    /// The text size commands, from the bundled widget set: the two A's
    /// are the standing mark for type scale.
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
    /// The fourth chevron, so a button can point any of the four ways the
    /// other three already do. Bundled set, no file of ours needed.
    pub const CHEVRON_UP: &str = "icons/chevron-up.svg";
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
    /// Sweeping a record away rather than deleting a thing: the stats
    /// window's clear, where the trash beside it would read as throwing
    /// out whatever row the pointer is near.
    pub const BROOM: &str = "icons/brush-cleaning.svg";
    pub const PENCIL: &str = "icons/pencil.svg";
    /// The bookmarks panel and the bookmark commands.
    pub const BOOKMARK: &str = "icons/bookmark.svg";
    /// The rating stars: the same outline both ways, the filled one with a
    /// solid fill so the pair reads set/unset at cell size.
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
    /// The tasks window and its status bar control: a checklist of jobs,
    /// ticked or waiting. A clock read as any of the other time surfaces,
    /// and the window is as much "what can I set going" as "what's running".
    pub const LIST_CHECKS: &str = "icons/list-checks.svg";
    /// The sleep timer once it's armed, since a plain clock would read as
    /// any of the other time surfaces.
    pub const BED: &str = "icons/bed.svg";
    pub const CALENDAR: &str = "icons/calendar.svg";
    pub const TAG: &str = "icons/tag.svg";
    pub const IMAGE: &str = "icons/image.svg";
    /// The backdrop preset picker: a look picked rather than a file
    /// chosen, which is what separates it from the plain image entry.
    pub const WAND_SPARKLES: &str = "icons/wand-sparkles.svg";
    /// The color grid's role link: an override following another app
    /// color instead of storing a literal.
    pub const LINK: &str = "icons/link.svg";
    pub const PALETTE: &str = "icons/palette.svg";
    pub const CONTRAST: &str = "icons/contrast.svg";
    pub const LAYOUT_DASHBOARD: &str = "icons/layout-dashboard.svg";
    pub const EYE: &str = "icons/eye.svg";
    /// The hidden half of a visibility toggle, the menubar's above all:
    /// a button that hides something wants the struck-through eye when
    /// it's already hidden. Bundled set, no file of ours needed.
    pub const EYE_OFF: &str = "icons/eye-off.svg";
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
    /// The banner's warning face, bundled set as well.
    pub const ALERT: &str = "icons/triangle-alert.svg";
    /// The Application menu's three ways out to the project: file a bug,
    /// join a thread, sit in the channel. The hash is the IRC one, since
    /// it draws a channel name.
    pub const BUG: &str = "icons/bug.svg";
    pub const MESSAGES: &str = "icons/messages-square.svg";
    pub const HASH: &str = "icons/hash.svg";
    pub const LAYOUT_GRID: &str = "icons/layout-grid.svg";
    pub const GALLERY: &str = "icons/gallery-horizontal-end.svg";
    pub const MOVE_VERTICAL: &str = "icons/move-vertical.svg";
    pub const MOVE_HORIZONTAL: &str = "icons/move-horizontal.svg";
    pub const AUDIO_LINES: &str = "icons/audio-lines.svg";
    pub const AUDIO_WAVEFORM: &str = "icons/audio-waveform.svg";
    /// The VU meter panel: a level gauge.
    pub const GAUGE: &str = "icons/gauge.svg";
    /// The oscilloscope panel: a traced waveform on a scope line.
    pub const ACTIVITY: &str = "icons/activity.svg";
    /// The spectrogram panel: stacked ripples, the waterfall of frequency
    /// over time rather than the single instant the band bars draw.
    pub const WAVES: &str = "icons/waves.svg";
    /// The mini-player toggle: shrink into the mini layout, grow back to
    /// the primary.
    pub const MINIMIZE: &str = "icons/minimize-2.svg";
    pub const MAXIMIZE: &str = "icons/maximize-2.svg";
    /// Exit fullscreen: corner brackets folding in, pairing with the
    /// square frame the maximize button shows otherwise. Bracket-style so
    /// it can't be mistaken for the mini toggle's shrink arrows.
    pub const FULLSCREEN_EXIT: &str = "icons/minimize.svg";
    /// The Window menu's Empty Window entry: a blank dock.
    pub const SQUARE_DASHED: &str = "icons/square-dashed.svg";
    /// The unfinished work: the settings sidebar's Development page, and
    /// the panel menu's experimental group. Both use the flask, so the two
    /// surfaces read as the same thing.
    pub const FLASK: &str = "icons/flask-conical.svg";
    /// The console, from the bundled widget set: a prompt in a frame.
    pub const SQUARE_TERMINAL: &str = "icons/square-terminal.svg";
    /// The settings sidebar's Keymap page, and the chord chips on it.
    pub const KEYBOARD: &str = "icons/keyboard.svg";
    /// The theme toggle panel's glyphs, the side a click switches to; both
    /// resolve from the bundled widget set, no file of ours needed.
    pub const SUN: &str = "icons/sun.svg";
    pub const MOON: &str = "icons/moon.svg";
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
    /// The composition panels: the group's split, the overlay panel's
    /// stacked layers, the drawer's docked edge, and the slide panel's
    /// back arrow (forward is CHEVRON_RIGHT; left resolves from the
    /// bundled widget set).
    pub const COLUMNS_2: &str = "icons/columns-2.svg";
    pub const LAYERS: &str = "icons/layers-2.svg";
    pub const PANEL_BOTTOM: &str = "icons/panel-bottom.svg";
    /// The rest of the panel-side family: the border edge toggles use
    /// all four, one per side the line can draw on.
    pub const PANEL_TOP: &str = "icons/panel-top.svg";
    pub const PANEL_LEFT: &str = "icons/panel-left.svg";
    pub const PANEL_RIGHT: &str = "icons/panel-right.svg";
    pub const CHEVRON_LEFT: &str = "icons/chevron-left.svg";
    /// The layout tree's per-panel lock toggle, pinned and free.
    pub const LOCK: &str = "icons/lock.svg";
    pub const LOCK_OPEN: &str = "icons/lock-open.svg";
    /// The drawer's pin, holding an open drawer out.
    pub const PIN: &str = "icons/pin.svg";
    /// The layout tree's reorder arrows and lift-out, from the bundled
    /// widget set.
    pub const ARROW_UP: &str = "icons/arrow-up.svg";
    pub const ARROW_DOWN: &str = "icons/arrow-down.svg";
    pub const ARROW_LEFT: &str = "icons/arrow-left.svg";
    /// The fourth arrow, bundled like the other three. Nothing in the
    /// tree draws it yet; it's here so a button pointing forward, the
    /// next-bookmark jump above all, isn't the one direction missing.
    pub const ARROW_RIGHT: &str = "icons/arrow-right.svg";
    /// The rox mark: a single-path logo, so it paints in the text color
    /// like any other svg element. Heads the empty launcher and the
    /// welcome window.
    pub const LOGO: &str = "app/rox-music.svg";

    /// Every icon the app draws, the set an icon picker can offer. It's the
    /// one enumerated list of the names the app asks for, so a picker reads
    /// it instead of guessing. The logo is left out: it's the brand mark,
    /// not an icon a button gets to wear.
    ///
    /// This order is the picker's order, so it runs glyph-first: the marks a
    /// button plausibly wears, then a tail of window and panel furniture.
    /// The chevrons, arrows and panel edges are real entries, just not what
    /// anyone is scrolling for, so they sit at the bottom rather than in the
    /// middle of the transport and library marks.
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
        // The chrome: window and panel furniture, sunk to the bottom of
        // the picker because a button rarely wants one.
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

/// Our embedded assets, checked before the bundled widget assets so a
/// same-named file here wins.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../rox/assets"]
#[include = "icons/**/*.svg"]
#[include = "app/rox-music.svg"]
#[include = "workspaces/**/*.json"]
#[include = "workspaces/**/*.png"]
#[include = "disc/*.png"]
pub struct Assets;

/// The shipped workspace bundles: every JSON file under `assets/workspaces`,
/// as `(file stem, raw bytes)`. The workspaces module names and parses them.
pub fn shipped_workspaces() -> Vec<(String, Cow<'static, [u8]>)> {
    shipped_json("workspaces/")
}

/// The preview picture shipped beside a workspace bundle for a theme side,
/// when one exists: `workspaces/<stem>_Dark.png` or `_Light.png` next to
/// the bundle's JSON, falling back to a plain `<stem>.png` serving both
/// sides, keyed by the file stem rather than the bundle's own name. The
/// welcome window's quick-start tiles draw the side the live theme picks;
/// a bundle with no picture shows a placeholder there.
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

/// The width over height of an embedded PNG asset, read straight from the
/// IHDR header. The quick-start tiles size a preview to its real scaled
/// height with it, so the hover pan sweeps the whole screenshot.
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

    /// The catalog is the list a picker offers, so every entry has to
    /// resolve to real bytes, ours or bundled. If one doesn't, a picked icon
    /// would draw as nothing.
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
