//! The Milkdrop panel: MilkDrop presets rendered by libprojectM on a thread
//! of its own and handed to gpui a frame at a time. [`rox_milkdrop::Engine`]
//! owns a private OpenGL context and publishes an rgba8 buffer; this file
//! puts that buffer on screen and exposes the knobs projectM takes.
//!
//! Each frame goes up as a user texture drawn through a one-pass shader
//! chain. An `img()` with a fresh `RenderImage` per frame would allocate
//! and drop through the sprite atlas every frame, and the chain lets a
//! surface shader sit over the frame like any other panel. The texture and
//! chain live in one window's renderer, so a pop-out registers a fresh pair.
//!
//! ADR 28, including the readback cost it takes on purpose.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, Div, EntityId, EventEmitter, FocusHandle, Focusable, MouseButton,
    PathPromptOptions, SharedString, Subscription, UserShaderChain, UserShaderId, UserShaderPass,
    UserTextureId, WeakEntity, Window, canvas, deferred, div, prelude::*, px,
};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_core::settings::{self as core_settings, MilkdropColor};
use rox_milkdrop::{Command, Engine, EngineOptions, Event, PresetLibrary, Rotation, Status};
use rox_panel_api::preset_browser::{
    PresetHost, Thumb, Thumbnails, find_folder_by_relative, folder_label, preset_label,
    relative_to_roots,
};
use rox_panel_kit::fade::{self, Fade};
use rox_panel_kit::grade::{self, Grade, GradeMode};

use crate::assets::icons;
use crate::design::{palette, tokens};
// Aliased the way `panels::shader` does: `panel::shader` is one letter
// from this crate's own `shader`.
use crate::panel::shader as surface;
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit, setting_row, toggle,
};
use crate::panel_settings;
use crate::settings::ui::{SECTION_GAP, section};

/// The floor keeps a sliver of a panel from asking for a 4x1 framebuffer;
/// the ceiling keeps a maximised 5K panel from a 30 MB-per-frame readback.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 4096;

const BANNER_HOLD: Duration = Duration::from_secs(2);

/// A cold driver can take a second to hand over a context, and the first
/// frame is two readbacks behind that.
const STALL_GRACE: Duration = Duration::from_secs(5);

/// Past a few seconds the fade reads as broken rather than calm.
const FADE_MAX: f32 = 5.0;

/// Enough for a red cover to read red across the frame, short of every
/// preset coming out the same colour.
const TINT_DEFAULT: f32 = 0.35;

/// A full ramp in or out, or half a turn of hue; a nearer cover takes
/// less. Slower than the fade on purpose: a hue that snaps on a track
/// change reads as a glitch.
const TINT_EASE: Duration = Duration::from_millis(1200);

/// Each root's mtime, checked by the Presets page before it trusts the
/// last scan. A pack dropped in moves it; a file added deep inside one
/// doesn't, which is what Rescan is for. A full walk is a stat per preset,
/// 150,000 for the big pack, so it can't run on a timer.
type RootsStamp = Vec<(PathBuf, Option<std::time::SystemTime>)>;

/// Below the floor it stops reading as motion; above the ceiling the
/// readback is the bottleneck.
const FPS_MIN: u32 = 10;
const FPS_MAX: u32 = 240;

/// Below a quarter the upscale looks like a mistake.
const SCALE_MIN: f32 = 0.25;
const SCALE_MAX: f32 = 1.0;

/// Not translated: they're repository names.
const PACKS: [(&str, &str); 3] = [
    (
        "presets-cream-of-the-crop",
        "https://github.com/projectM-visualizer/presets-cream-of-the-crop",
    ),
    (
        "presets-milkdrop-original",
        "https://github.com/projectM-visualizer/presets-milkdrop-original",
    ),
    (
        "presets-milkdrop-texture-pack",
        "https://github.com/projectM-visualizer/presets-milkdrop-texture-pack",
    ),
];

/// The one pass that puts a frame on screen: sample, aspect-fit, letterbox,
/// fade. The fit only matters during the one-frame resize debounce, when
/// the texture and the panel disagree.
///
/// The fade rides `signals[0].x` as premultiplied alpha, the contract every
/// user shader writes under. The region blends `src + dst * (1 - src.a)`, so
/// a fade of `f` lands `f` of the way from the body's own `bg_root()` to the
/// frame and zero is the identity. That's why there's no background colour
/// to pass in.
///
/// `signals[0].y` and `.z` are the tint hue and amount from
/// [`MilkdropPanel::step_tint`]; the grade's slots follow, filled by
/// [`Grade::write`], with the maths in [`rox_panel_kit::grade`]. The tint
/// rotates hue in Oklab rather than mixing toward the cover colour, so each
/// pixel keeps its lightness and chroma and the preset keeps its palette's
/// shape. The sampler returns linear light (the texture is
/// `Rgba8UnormSrgb`), the space Oklab is defined from.
const FRAME_WGSL: &str = "
fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let fade = clamp(params.signals[0].x, 0.0, 1.0);
    let frame_size = vec2<f32>(textureDimensions(frame));
    let bounds = max(params.resolution, vec2<f32>(1.0, 1.0));
    let fit = min(bounds.x / frame_size.x, bounds.y / frame_size.y);
    let fitted = frame_size * fit;
    let margin = (bounds - fitted) * 0.5;
    var source = (uv * bounds - margin) / fitted;
    // Slots 14 and 15 mirror the sample, which mirrors the frame.
    if (params.signals[3].z > 0.5) {
        source.x = 1.0 - source.x;
    }
    if (params.signals[3].w > 0.5) {
        source.y = 1.0 - source.y;
    }
    if (source.x < 0.0 || source.x > 1.0 || source.y < 0.0 || source.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, fade);
    }
    let graded = grade(textureSample(frame, samp, source).rgb);
    return vec4<f32>(graded * fade, fade);
}
";

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MilkdropConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// By file name only, so a workspace carries the pick to another machine.
    /// Older layouts hold the whole path; the name is taken off it on load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
    /// Stay on the current preset. Renamed on the wire: the flattened
    /// [`PanelChrome`] already writes `locked` for the dock pin.
    #[serde(rename = "preset_locked")]
    pub locked: bool,
    pub duration_secs: f64,
    pub beat_sensitivity: f32,
    /// A loud enough beat cuts to the next preset without waiting out the
    /// duration.
    pub hard_cuts: bool,
    pub fps: u32,
    /// Fraction of the panel's device pixels; below 1.0 the frame is upscaled.
    pub scale: f32,
    pub show_preset_name: bool,
    /// Only means anything with `show_preset_name` on.
    pub name_always: bool,
    pub show_controls: bool,
    /// Off holds the last frame on pause or stop; on fades to the panel
    /// background over `fade_secs`. Kept apart from the duration so switching
    /// the fade back on returns the picked one.
    pub fade: bool,
    /// Zero cuts straight to the background. Only read with `fade` on.
    pub fade_secs: f32,
    /// Keep the worker running while the panel has focus. Only offered with
    /// `fade` off, and off by default because focus is sticky; see
    /// `park_or_resume`.
    pub run_focused: bool,
    /// 0 for none, 1 for every hue landing on the cover's.
    pub tint_strength: f32,
    /// Bounds the rotation to one folder, as a forward-slash path under the
    /// scan root: `cream/Fractal`. Next, Previous and the timed switch stay
    /// inside it; an explicit pick doesn't. Resolved by path and then by
    /// name, so a workspace finds the pack wherever it sits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_folder: Option<String>,
    /// The folder pick stays underneath so switching this off returns to it.
    /// With nothing starred the worker walks the whole library.
    pub favorites_only: bool,
    pub color: MilkdropColor,
}

impl Default for MilkdropConfig {
    fn default() -> Self {
        MilkdropConfig {
            chrome: PanelChrome::default(),
            preset: None,
            flip_horizontal: false,
            flip_vertical: false,
            locked: false,
            duration_secs: 30.0,
            beat_sensitivity: 1.0,
            hard_cuts: true,
            fps: 60,
            scale: 1.0,
            show_preset_name: true,
            name_always: false,
            show_controls: true,
            fade: false,
            fade_secs: 1.0,
            run_focused: false,
            tint_strength: TINT_DEFAULT,
            rotation_folder: None,
            favorites_only: false,
            color: MilkdropColor::default(),
        }
    }
}

/// The rotation picker's one value. The config keeps two fields so a pick
/// survives a round trip through the other.
#[derive(Clone, Debug, PartialEq)]
enum RotationChoice {
    All,
    Favorites,
    Folder(PathBuf),
}

fn grade_mode(color: MilkdropColor) -> GradeMode {
    match color {
        MilkdropColor::Preset => GradeMode::Preset,
        MilkdropColor::Theme => GradeMode::Theme,
        MilkdropColor::Palette => GradeMode::Palette,
        MilkdropColor::Cover => GradeMode::Cover,
    }
}

/// Device pixels times the config's scale, each side clamped on its own,
/// so a very wide, very short panel gets a squarer frame that the shader
/// letterboxes.
fn render_size(width: f32, height: f32, scale_factor: f32, scale: f32) -> (u32, u32) {
    let side = |logical: f32| {
        let device = logical * scale_factor * scale;
        // NaN and negatives land on the floor: f32 `max` takes the other operand
        // when one is NaN, and the cast saturates.
        (device.round().max(0.0) as u32).clamp(MIN_SIDE, MAX_SIDE)
    };
    (side(width), side(height))
}

/// State rather than a straight read of the seed, so a track change swings
/// the hue across and a stop lets the amount down instead of jumping.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TintAim {
    /// Oklab hue in radians, meaningless while `amount` is zero.
    hue: f32,
    amount: f32,
}

/// Turns `from` toward `to` by a fraction of the shortest arc. The fold
/// picks the short way: the long way round walks the colour through every
/// hue in between.
fn turned_hue(from: f32, to: f32, amount: f32) -> f32 {
    let apart = to - from;
    let delta = apart - std::f32::consts::TAU * (apart / std::f32::consts::TAU).round();
    from + delta * amount.clamp(0.0, 1.0)
}

/// `step` is the fraction of the ease this frame covers, so the result
/// doesn't depend on frame rate. A tint that isn't showing takes a new hue
/// whole: easing it at zero amount walks through colours the cover never
/// had.
fn ease_tint(current: TintAim, goal: Option<f32>, step: f32) -> TintAim {
    let step = step.clamp(0.0, 1.0);
    match goal {
        None => TintAim {
            hue: current.hue,
            amount: (current.amount - step).max(0.0),
        },
        Some(hue) if current.amount <= 0.0 => TintAim { hue, amount: step },
        Some(hue) => {
            // A constant angular rate. Chipping at the remainder never lands, and
            // the panel keeps asking for frames over a colour that stopped moving.
            let arc = turned_hue(current.hue, hue, 1.0) - current.hue;
            let travel = (step * std::f32::consts::PI).min(arc.abs());
            TintAim {
                hue: current.hue + arc.signum() * travel,
                amount: (current.amount + step).min(1.0),
            }
        }
    }
}

/// Older layouts stored the path, whose last component is the same name.
fn preset_name(stored: &str) -> String {
    Path::new(stored)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| stored.to_string())
}

/// Every pack writes the author into the file name, so the first match
/// is the preset.
fn find_by_name(presets: &[PathBuf], name: &str) -> Option<PathBuf> {
    presets
        .iter()
        .find(|preset| {
            preset
                .file_name()
                .is_some_and(|file| file.to_string_lossy() == name)
        })
        .cloned()
}

struct Target {
    /// A pop-out moves the panel to another window, and neither the texture
    /// nor the chain travels.
    window: u64,
    texture: UserTextureId,
    width: u32,
    height: u32,
    /// Registered on the first paint after the texture, dropped with it on a
    /// resize.
    chain: Option<UserShaderId>,
    /// The engine hands back only frames past this, so a UI frame with no new
    /// render costs one atomic load.
    last_seq: u64,
}

/// Behind a mutex because the paint closure has a `&mut App` and no
/// handle on the panel.
#[derive(Default)]
struct Draw {
    /// `None` until the first paint with a real size, which is why the engine
    /// isn't started in the constructor.
    size: Option<(u32, u32)>,
    /// A second frame at the same size commits it, so an edge drag doesn't
    /// reallocate the framebuffer per pixel.
    pending: Option<(u32, u32)>,
    target: Option<Target>,
    error: Option<String>,
    /// Zero after the grace is a worker running with nothing to show, which
    /// the body reports.
    frames: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Snapshot {
    pub preset: Option<PathBuf>,
    pub locked: bool,
    /// How many presets the last scan found.
    pub presets: usize,
    /// `all`, `favorites`, or the rotation folder's path.
    pub rotation: String,
    /// projectM's message for the last refused preset, until another lands.
    pub failed: Option<String>,
    /// The worker's or the window's failure, when there's no frame coming.
    pub error: Option<String>,
    /// `idle` before the first paint spawns the worker, then `starting`,
    /// `running`, or `failed`.
    pub engine: &'static str,
    pub renderer: Option<String>,
    pub projectm_version: Option<String>,
    pub frames: u64,
}

pub struct MilkdropPanel {
    state: AppState,
    config: MilkdropConfig,
    /// `None` until the first paint reports a size.
    engine: Option<Engine>,
    /// Scanned on the first render: restoring a layout builds every panel,
    /// and a pack is a few thousand `stat` calls.
    library: Option<PresetLibrary>,
    scanned_stamp: Option<RootsStamp>,
    /// Worked out with the scan: a walk of the whole library per keystroke is
    /// lag.
    folders: Vec<PathBuf>,
    folder_options: Vec<(RotationChoice, SharedString)>,
    /// The generation of the app-wide favorites and folders this panel last
    /// acted on. A move means another panel or the settings window edited them.
    lists_gen: u64,
    /// Runtime only: the config keeps the name.
    current: Option<PathBuf>,
    draw: Arc<Mutex<Draw>>,
    banner: Option<(String, Instant)>,
    failed: Option<String>,
    /// Set at the bottom of a fade-out or on the first frame of a hold,
    /// cleared on play.
    parked: bool,
    hovered: bool,
    /// When the panel last asked for frames. Stall notices count from here,
    /// so a panel parked for an hour isn't called stuck the moment it wakes.
    since: Option<Instant>,
    /// Whether the worker has run yet. Until it has, a hold reads as a cut:
    /// holding nothing pins an empty texture at full opacity.
    held: bool,
    /// Driven by the play state, and the clock the park waits on.
    fade: Fade,
    /// Stepped by wall time, so a panel at 30 fps eases as fast as one at 60.
    tint: TintAim,
    tint_at: Instant,
    fps_scrub: ScrubState,
    duration_scrub: ScrubState,
    sensitivity_scrub: ScrubState,
    scale_scrub: ScrubState,
    fade_scrub: ScrubState,
    tint_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel on every pump tick, which carries the play-state change
    /// that parks the worker.
    _player_changed: Subscription,
    /// Built on the first render: it needs a window. A parked panel renders
    /// nothing that asks for a frame, so without this a click wouldn't wake it.
    _focus_wake: Option<Subscription>,
    /// Takes the engine down on quit; see [`MilkdropPanel::new`].
    _quit: Subscription,
    /// Latched by the quit hook so a late render doesn't start another worker.
    quitting: bool,
}

impl MilkdropPanel {
    pub fn new(state: AppState, mut config: MilkdropConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        // gpui doesn't unwind its entities when the app loop returns, so without
        // this the worker is still rendering while glibc runs Mesa's exit
        // handlers: a segfault inside the driver. The hang-up happens here and
        // the wait in the future, so several engines going down overlap rather
        // than stack. `rox_milkdrop`'s exit guard is the backstop.
        let _quit = cx.on_app_quit(|panel: &mut Self, _| {
            panel.quitting = true;
            let engine = panel.engine.take();
            let at = Instant::now();
            if let Some(engine) = engine.as_ref() {
                engine.stop();
            }
            async move {
                let Some(engine) = engine else {
                    return;
                };
                if engine.wait() {
                    log::info!(
                        "milkdrop panel: engine stood down {} us after the hang-up",
                        at.elapsed().as_micros()
                    );
                } else {
                    log::warn!("milkdrop panel: engine still up at quit");
                }
            }
        });
        // Older layouts hold a path here.
        config.preset = config.preset.as_deref().map(preset_name);
        MilkdropPanel {
            state,
            config,
            engine: None,
            library: None,
            scanned_stamp: None,
            folders: Vec::new(),
            folder_options: Vec::new(),
            lists_gen: core_settings::milkdrop_gen(),
            current: None,
            draw: Arc::new(Mutex::new(Draw::default())),
            banner: None,
            failed: None,
            parked: false,
            hovered: false,
            since: None,
            held: false,
            // Fully visible: the first render with nothing playing takes it down.
            fade: Fade::default(),
            // Untinted, so the first cover eases in.
            tint: TintAim::default(),
            tint_at: Instant::now(),
            fps_scrub: ScrubState::default(),
            duration_scrub: ScrubState::default(),
            sensitivity_scrub: ScrubState::default(),
            scale_scrub: ScrubState::default(),
            fade_scrub: ScrubState::default(),
            tint_scrub: ScrubState::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _focus_wake: None,
            _quit,
            quitting: false,
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        core_settings::milkdrop_scan_roots()
    }

    fn library(&mut self) -> &PresetLibrary {
        if self.library.is_none() {
            self.rescan();
        }
        self.library.as_ref().expect("just scanned")
    }

    fn rescan(&mut self) {
        let textures = rox_core::settings::milkdrop_dir().join("textures");
        let textures = textures.is_dir().then_some(textures);
        let mut library = PresetLibrary::scan(&self.roots(), textures);
        // A favorite starred from a panel with other roots joins the list rather
        // than falling out of the favorites rotation.
        library.extend(&core_settings::milkdrop_favorites());
        self.scanned_stamp = Some(self.roots_stamp());
        // The worker took the library by value, so a changed scan has to go out
        // or the panel keeps rotating the old one.
        let changed = self.library.as_ref() != Some(&library);
        self.library = Some(library);
        self.refresh_folders();
        if changed {
            let library = self.library.clone().expect("just set");
            self.send(Command::SetLibrary {
                library,
                rotation: self.rotation(),
            });
        }
    }

    fn refresh_folders(&mut self) {
        let Some(library) = self.library.as_ref() else {
            return;
        };
        let roots = library.roots().to_vec();
        self.folders = library.folders();
        self.folder_options = self
            .folders
            .iter()
            .map(|folder| {
                let label = SharedString::from(folder_label(folder, &roots));
                (RotationChoice::Folder(folder.clone()), label)
            })
            .collect();
    }

    fn roots_stamp(&self) -> RootsStamp {
        self.roots()
            .into_iter()
            .map(|root| {
                let modified = std::fs::metadata(&root)
                    .and_then(|meta| meta.modified())
                    .ok();
                (root, modified)
            })
            .collect()
    }

    /// What the Presets page calls, so a pack dropped in shows up on the next
    /// visit without walking the library per draw.
    fn rescan_if_stale(&mut self) {
        if self.scanned_stamp.as_ref() != Some(&self.roots_stamp()) {
            self.rescan();
        }
    }

    /// Favorites win over the folder while the switch is on.
    fn rotation(&self) -> Rotation {
        if self.config.favorites_only {
            return Rotation::Set(core_settings::milkdrop_favorites());
        }
        match self.rotation_dir() {
            Some(folder) => Rotation::Folder(folder),
            None => Rotation::All,
        }
    }

    /// Found by path under a root, then by name. None rotates everything, as
    /// a deleted folder always did.
    fn rotation_dir(&self) -> Option<PathBuf> {
        let relative = self.config.rotation_folder.as_deref()?;
        let library = self.library.as_ref()?;
        find_folder_by_relative(&self.folders, library.roots(), relative)
    }

    fn rotation_choice(&self) -> RotationChoice {
        if self.config.favorites_only {
            return RotationChoice::Favorites;
        }
        match self.rotation_dir() {
            Some(folder) => RotationChoice::Folder(folder),
            None => RotationChoice::All,
        }
    }

    fn set_rotation(&mut self, choice: RotationChoice, cx: &mut Context<Self>) {
        match choice {
            RotationChoice::All => {
                self.config.favorites_only = false;
                self.config.rotation_folder = None;
            }
            RotationChoice::Favorites => self.config.favorites_only = true,
            RotationChoice::Folder(folder) => {
                self.config.favorites_only = false;
                // A favorite from outside every root has no path under one; its name is
                // all there is.
                self.config.rotation_folder =
                    relative_to_roots(&folder, &self.roots()).or_else(|| {
                        folder
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    });
            }
        }
        self.send(Command::SetRotation(self.rotation()));
        cx.notify();
    }

    fn set_favorites_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.config.favorites_only = on;
        self.send(Command::SetRotation(self.rotation()));
        cx.notify();
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.current.clone()
    }

    fn current_is_favorite(&self) -> bool {
        self.current_path()
            .is_some_and(|path| core_settings::is_milkdrop_favorite(&path))
    }

    /// The write goes to the app-wide list. The next render sees the
    /// generation move, the same path another panel's change takes.
    fn toggle_favorite(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let on = !core_settings::is_milkdrop_favorite(&path);
        core_settings::set_milkdrop_favorite(&path, on);
        cx.notify();
    }

    fn reveal_preset(&self, cx: &mut App) {
        if let Some(path) = self.current_path() {
            cx.reveal_path(&path);
        }
    }

    /// Folds the favorites into the held library, which stats the favorites
    /// alone: a full rescan per star is a click that hangs. The rotation is
    /// re-sent either way, since a favorites rotation is the list.
    fn follow_lists(&mut self) {
        let generation = core_settings::milkdrop_gen();
        if generation == self.lists_gen {
            return;
        }
        self.lists_gen = generation;
        // A folder edit is the one thing that earns a rescan.
        let roots = self.roots();
        if self
            .library
            .as_ref()
            .is_some_and(|library| library.roots() != roots.as_slice())
        {
            self.rescan();
            self.send(Command::SetRotation(self.rotation()));
            return;
        }
        let favorites = core_settings::milkdrop_favorites();
        if let Some(library) = self.library.as_mut() {
            let before = library.presets().len();
            library.extend(&favorites);
            if library.presets().len() != before {
                let library = library.clone();
                self.refresh_folders();
                self.send(Command::SetLibrary {
                    library,
                    rotation: self.rotation(),
                });
            }
        }
        if self.config.favorites_only {
            self.send(Command::SetRotation(self.rotation()));
        }
    }

    /// Nothing else creates the folder, and the platform refuses a missing
    /// path.
    fn open_presets_folder(cx: &mut App) {
        let folder = rox_core::settings::milkdrop_dir().join("presets");
        std::fs::create_dir_all(&folder).ok();
        cx.open_with_system(&folder);
    }

    /// A panel whose engine hasn't started drops commands rather than
    /// queueing them against a size nobody has measured.
    fn send(&self, command: Command) {
        if let Some(engine) = self.engine.as_ref() {
            engine.send(command);
        }
    }

    /// The library goes in by value, a snapshot rather than a lock on the
    /// render loop. A rescan sends a new one with `Command::SetLibrary`.
    fn start(&mut self, size: (u32, u32), cx: &mut Context<Self>) {
        let library = self.library().clone();
        // The saved preset goes in with the spawn so the worker doesn't shuffle
        // one and switch a frame later.
        let preset = self.restore_path();
        let engine = Engine::spawn(EngineOptions {
            feed: self.state.player.read(cx).feed(),
            library,
            preset,
            fps: self.config.fps,
            width: size.0,
            height: size.1,
        });
        // Pushed at startup so a restored panel comes up as it was left.
        engine.send(Command::SetPresetDuration(self.config.duration_secs));
        engine.send(Command::SetBeatSensitivity(self.config.beat_sensitivity));
        engine.send(Command::SetHardCut(self.config.hard_cuts));
        engine.send(Command::SetLocked(self.config.locked));
        // Without it a restored folder pick shows on the page while the worker
        // walks the whole library.
        engine.send(Command::SetRotation(self.rotation()));
        self.engine = Some(engine);
        self.since = Some(Instant::now());
    }

    /// Found in the scan by name, so a workspace lands on the same preset
    /// wherever the pack lives.
    fn restore_path(&mut self) -> Option<PathBuf> {
        let name = self.config.preset.clone()?;
        find_by_name(self.library().presets(), &name)
    }

    fn note_preset(&mut self, path: &Path) {
        self.config.preset = Some(preset_name(&path.to_string_lossy()));
        self.current = Some(path.to_path_buf());
    }

    /// Zero with the switch off.
    fn fade_duration(&self) -> Duration {
        if !self.config.fade {
            return Duration::ZERO;
        }
        Duration::from_secs_f32(self.config.fade_secs.clamp(0.0, FADE_MAX))
    }

    /// 1.0 fully drawn, 0.0 fully gone.
    fn opacity(&self) -> f32 {
        self.fade.opacity(self.fade_duration())
    }

    /// Keeps the panel asking for frames after the event that started the
    /// fade.
    fn fading(&self) -> bool {
        self.fade.running(self.fade_duration())
    }

    /// Starts from wherever the current fade got to, so a stop mid fade-in
    /// turns around there.
    fn fade_to(&mut self, to: f32) {
        let duration = self.fade_duration();
        self.fade.retarget(to, duration);
    }

    /// Playing runs the worker. A pause or a stop holds the last frame or
    /// fades out, per the config, and parks the worker under it.
    ///
    /// A held frame costs nothing: the texture keeps the last render, so
    /// later frames sample it with no render, readback or upload. On the fade
    /// path the pause goes out at the bottom of the fade, since a parked
    /// worker publishes nothing and would freeze the picture mid-fade.
    ///
    /// `run_focused` keeps a held, focused panel running. It's a switch
    /// because focus is sticky: the dock focuses the active tab, so a panel
    /// stays focused long after use, and a visual cycling under a paused
    /// track reads as the pause not having taken.
    fn park_or_resume(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            return;
        }
        let playing = self.state.player.read(cx).is_playing();
        let hold = !self.config.fade;
        // Focus only stands in for the audio on a held frame; a fading panel
        // follows the audio alone.
        let focused = hold && self.config.run_focused && self.focus.is_focused(window);
        if playing || focused {
            self.held = true;
            self.fade_to(1.0);
            if self.parked {
                self.parked = false;
                self.since = Some(Instant::now());
                self.send(Command::Resume);
            }
            return;
        }
        // Never having run is the hold that isn't: an empty texture at full
        // opacity is a black panel.
        if hold && self.held {
            self.fade_to(1.0);
            if !self.parked {
                self.parked = true;
                self.send(Command::Pause);
            }
            return;
        }
        self.fade_to(0.0);
        if !self.parked && self.opacity() <= 0.0 {
            self.parked = true;
            self.send(Command::Pause);
        }
    }

    /// Returns whether the tint still has somewhere to go. The seed is the
    /// palette's per-player store, written on every track change, so the
    /// frame follows the cover whatever the theme mode, and a colourless
    /// cover eases off like a stop.
    fn step_tint(&mut self) -> bool {
        let goal = palette::seed(self.state.player.entity_id())
            .and_then(|seed| seed.primary)
            .map(|primary| palette::rgba_to_oklch(primary).2)
            // Strength zero lets go of the hue the same way a stop does.
            .filter(|_| self.config.tint_strength > 0.0);
        let elapsed = self.tint_at.elapsed();
        self.tint_at = Instant::now();
        let step = elapsed.as_secs_f32() / TINT_EASE.as_secs_f32();
        self.tint = ease_tint(self.tint, goal, step);
        match goal {
            // Folded the way the step folds it, so a hue wrapped past a full turn
            // is no further from its goal.
            Some(hue) => {
                self.tint.amount < 1.0
                    || (turned_hue(self.tint.hue, hue, 1.0) - self.tint.hue).abs() > 1e-3
            }
            None => self.tint.amount > 0.0,
        }
    }

    fn drain_events(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        for event in engine.take_events() {
            match event {
                Event::PresetChanged(path) => {
                    self.note_preset(&path);
                    self.failed = None;
                    // A preset that runs here has earned another thumbnail attempt.
                    thumbnails().loaded(&path);
                    if self.config.show_preset_name {
                        self.banner = Some((preset_label(&path), Instant::now()));
                    }
                }
                Event::PresetFailed { path, message } => {
                    log::warn!("milkdrop preset {} failed: {message}", path.display());
                    self.failed = Some(message.clone());
                    self.banner = Some((message, Instant::now()));
                }
            }
        }
    }

    fn error(&self) -> Option<String> {
        if let Some(error) = self.draw.lock().unwrap().error.clone() {
            return Some(error);
        }
        match self.engine.as_ref().map(Engine::status) {
            Some(Status::Failed(message)) => Some(message),
            _ => None,
        }
    }

    /// A start still waiting on the driver past the grace, or a running
    /// engine that never delivered a frame. Unannounced, either is a black
    /// panel that looks like the feature working with the lights off.
    fn stall(&self) -> Option<(SharedString, SharedString)> {
        let engine = self.engine.as_ref()?;
        if self.parked || self.since?.elapsed() < STALL_GRACE {
            return None;
        }
        match engine.status() {
            Status::Starting => Some((
                rox_i18n::t!("milkdrop-still-starting"),
                rox_i18n::t!("milkdrop-still-starting-detail"),
            )),
            Status::Running {
                renderer,
                gl_version,
                ..
            } if self.draw.lock().unwrap().frames == 0 => Some((
                rox_i18n::t!("milkdrop-no-frames"),
                rox_i18n::t!(
                    "milkdrop-no-frames-detail",
                    renderer = renderer,
                    version = gl_version
                ),
            )),
            _ => None,
        }
    }

    pub fn load_preset(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.note_preset(&path);
        self.send(Command::LoadPreset { path, smooth: true });
        cx.notify();
    }

    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        self.config.locked = locked;
        self.send(Command::SetLocked(locked));
        cx.notify();
    }

    pub fn rescan_presets(&mut self, cx: &mut Context<Self>) {
        self.rescan();
        cx.notify();
    }

    /// A hard cut, the same command the context menu's Random sends.
    pub fn next_preset(&self) {
        self.send(Command::NextPreset { smooth: false });
    }

    pub fn previous_preset(&self) {
        self.send(Command::PreviousPreset { smooth: false });
    }

    /// As projectM drew it, before the tint, the grade, and the flips.
    pub fn latest_frame(&self) -> Option<rox_milkdrop::Frame> {
        self.engine.as_ref()?.frame_after(0)
    }

    /// For the control socket's debug scope (ADR 22). It reflects the events
    /// the last paint drained, so a load reads back once a frame has drawn
    /// after it.
    pub fn snapshot(&mut self) -> Snapshot {
        let presets = self.library().presets().len();
        let error = self.error();
        let (engine, renderer, projectm_version) = match self.engine.as_ref().map(Engine::status) {
            None => ("idle", None, None),
            Some(Status::Starting) => ("starting", None, None),
            Some(Status::Running {
                renderer,
                projectm_version,
                ..
            }) => ("running", Some(renderer), Some(projectm_version)),
            Some(Status::Failed(_)) => ("failed", None, None),
        };
        let rotation = match self.rotation_choice() {
            RotationChoice::All => "all".to_string(),
            RotationChoice::Favorites => "favorites".to_string(),
            RotationChoice::Folder(folder) => folder.to_string_lossy().into_owned(),
        };
        Snapshot {
            preset: self.current.clone(),
            locked: self.config.locked,
            presets,
            rotation,
            failed: self.failed.clone(),
            error,
            engine,
            renderer,
            projectm_version,
            frames: self.draw.lock().unwrap().frames,
        }
    }

    /// Directories only: a pack is a folder, and a file would be a preset.
    fn pick_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            // The list is app-wide. The write moves the generation, and this panel's
            // next render rescans like every other.
            this.update(cx, |_, cx| {
                let mut roots = core_settings::milkdrop_roots();
                if !roots.contains(&path) {
                    roots.push(path);
                    core_settings::set_milkdrop_roots(roots);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_root(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut roots = core_settings::milkdrop_roots();
        if index < roots.len() {
            roots.remove(index);
            core_settings::set_milkdrop_roots(roots);
            cx.notify();
        }
    }
}

/// One for the app: one tiny engine is plenty, and two would race for
/// the same files.
pub fn thumbnails() -> Arc<dyn Thumbnails> {
    static THUMBS: std::sync::OnceLock<Arc<ThumbService>> = std::sync::OnceLock::new();
    THUMBS
        .get_or_init(|| {
            let dir = core_settings::milkdrop_dir();
            let textures = dir.join("textures");
            let textures = textures.is_dir().then_some(textures);
            Arc::new(ThumbService(rox_milkdrop::thumbs::Thumbnailer::new(
                dir.join("thumbs"),
                textures,
            )))
        })
        .clone()
}

struct ThumbService(rox_milkdrop::thumbs::Thumbnailer);

impl Thumbnails for ThumbService {
    fn thumb(&self, preset: &Path) -> Thumb {
        match self.0.thumb(preset) {
            rox_milkdrop::thumbs::Thumb::Ready(path) => Thumb::Ready(path),
            rox_milkdrop::thumbs::Thumb::Pending => Thumb::Pending,
            rox_milkdrop::thumbs::Thumb::Failed(message) => Thumb::Failed(message),
        }
    }

    fn want(&self, presets: Vec<PathBuf>) {
        self.0.want(presets);
    }

    fn generation(&self) -> u64 {
        self.0.generation()
    }

    fn loaded(&self, preset: &Path) {
        self.0.loaded(preset);
    }
}

/// Weak, so a picker left open over a closed panel doesn't keep it alive.
struct PanelHost(WeakEntity<MilkdropPanel>);

impl PresetHost for PanelHost {
    fn title(&self, cx: &App) -> SharedString {
        self.0
            .upgrade()
            .and_then(|panel| panel.read(cx).config.chrome.title.clone())
            .map(SharedString::from)
            .unwrap_or_else(|| rox_i18n::t!("panel-title-milkdrop"))
    }

    fn presets(&self, cx: &mut App) -> Vec<PathBuf> {
        self.0
            .update(cx, |panel, _| panel.library().presets().to_vec())
            .unwrap_or_default()
    }

    fn current(&self, cx: &App) -> Option<PathBuf> {
        self.0
            .upgrade()
            .and_then(|panel| panel.read(cx).current_path())
    }

    fn pick(&self, path: PathBuf, cx: &mut App) {
        self.0
            .update(cx, |panel, cx| panel.load_preset(path, cx))
            .ok();
    }

    fn random(&self, cx: &mut App) {
        if let Some(panel) = self.0.upgrade() {
            panel.read(cx).send(Command::NextPreset { smooth: false });
        }
    }

    fn watch(
        &self,
        wake: std::rc::Rc<dyn Fn(&mut App)>,
        gone: std::rc::Rc<dyn Fn(&mut App)>,
        cx: &mut App,
    ) -> Vec<Subscription> {
        let Some(panel) = self.0.upgrade() else {
            gone(cx);
            return Vec::new();
        };
        vec![
            cx.observe(&panel, move |_, cx| wake(cx)),
            cx.observe_release(&panel, move |_, cx| gone(cx)),
        ]
    }
}

#[derive(Clone, Copy)]
struct Look {
    /// Slot 0: the fade, already shaped.
    fade: f32,
    /// Slot 1: the hue the frame turns toward, in radians.
    hue: f32,
    /// Slot 2: how far it turns, the ease and the strength multiplied out.
    tint: f32,
    /// Slots 3 to 13: the theme grade.
    grade: Grade,
    /// Slots 14 and 15: the mirrors.
    flip_horizontal: bool,
    flip_vertical: bool,
}

const SLOT_FLIP_H: usize = 14;
const SLOT_FLIP_V: usize = 15;

/// Sizes the texture, uploads the newest render, and records the pass.
#[allow(clippy::too_many_arguments)]
fn paint(
    bounds: gpui::Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
    scale: f32,
    look: Look,
    animate: bool,
    engine: Option<&Engine>,
    draw: &Mutex<Draw>,
    panel: EntityId,
) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    let want = render_size(
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        window.scale_factor(),
        scale,
    );
    let mut draw = draw.lock().unwrap();

    // The first real size starts the worker, and only the panel can do
    // that: this closure has no handle on it.
    if draw.size != Some(want) {
        draw.size = Some(want);
        cx.notify(panel);
    }
    let Some(engine) = engine else {
        return;
    };

    let window_id = window.window_handle().window_id().as_u64();
    let fits = draw
        .target
        .as_ref()
        .is_some_and(|target| target.window == window_id && (target.width, target.height) == want);
    if fits {
        // A drag back to the start leaves a `pending` size nothing will commit,
        // and a later resize to it would skip its debounce.
        draw.pending = None;
    } else {
        // A target in another window is a pop-out: nothing is on screen until a
        // new one exists. Only a real size change waits out the debounce.
        let moved = draw
            .target
            .as_ref()
            .is_none_or(|target| target.window != window_id);
        if moved || draw.pending == Some(want) {
            draw.pending = None;
            if let Some(old) = draw.target.take() {
                // Only this window's registry can free it. A texture left in the window
                // the panel popped out of dies with that window.
                if old.window == window_id {
                    window.release_user_texture(old.texture);
                }
            }
            match window.register_dynamic_texture(want.0, want.1) {
                Ok(texture) => {
                    draw.error = None;
                    draw.target = Some(Target {
                        window: window_id,
                        texture,
                        width: want.0,
                        height: want.1,
                        chain: None,
                        last_seq: 0,
                    });
                    engine.send(Command::Resize {
                        width: want.0,
                        height: want.1,
                    });
                }
                Err(message) => {
                    if draw.error.as_deref() != Some(message.as_str()) {
                        draw.error = Some(message);
                        cx.notify(panel);
                    }
                }
            }
        } else {
            // First sighting of this size. Ask for another frame so the second
            // arrives with nothing else moving, and keep drawing the old target:
            // a blanked frame reads as a flicker.
            draw.pending = Some(want);
            window.request_animation_frame();
        }
    }

    // A frame from before a resize is dropped rather than stretched. Its seq
    // still moves on, so it isn't re-fetched every frame.
    let uploaded = {
        let Some(target) = draw.target.as_mut() else {
            return;
        };
        match engine.frame_after(target.last_seq) {
            Some(frame) => {
                target.last_seq = frame.seq;
                let fits = frame.width == target.width && frame.height == target.height;
                if fits
                    && let Err(message) = window.update_user_texture(target.texture, frame.rgba8)
                {
                    draw.error = Some(message);
                    cx.notify(panel);
                    return;
                }
                fits
            }
            None => false,
        }
    };
    if uploaded {
        draw.frames += 1;
    }
    let Some(target) = draw.target.as_mut() else {
        return;
    };

    if target.chain.is_none() {
        let chain = UserShaderChain {
            passes: vec![UserShaderPass {
                name: "main".to_string(),
                source: grade::wgsl(FRAME_WGSL),
                scale: 1.0,
            }],
            assets: vec![("frame".to_string(), target.texture)],
        };
        match window.register_user_shader_chain(&chain) {
            Ok(shader) => {
                target.chain = Some(shader);
                draw.error = None;
            }
            Err(message) => {
                if draw.error.as_deref() != Some(message.as_str()) {
                    draw.error = Some(message);
                    cx.notify(panel);
                }
                return;
            }
        }
    }
    let Some(shader) = draw.target.as_ref().and_then(|target| target.chain) else {
        return;
    };

    // A chain that binds an asset only runs as a screen pass, so this is
    // always the region branch. The entity id keys the region's scratch
    // texture.
    let meta = surface::meta_slots(window, cx);
    // Element opacity doesn't reach a shader region, so the fade goes
    // through the pass.
    let mut signals = [0.0f32; 16];
    signals[grade::SLOT_FADE] = look.fade;
    signals[grade::SLOT_HUE] = look.hue;
    signals[grade::SLOT_TINT] = look.tint;
    look.grade.write(&mut signals);
    signals[SLOT_FLIP_H] = if look.flip_horizontal { 1.0 } else { 0.0 };
    signals[SLOT_FLIP_V] = if look.flip_vertical { 1.0 } else { 0.0 };
    window.paint_screen_shader(bounds, shader, panel.as_u64(), signals, meta);

    if animate {
        // A docked panel renders cached and replays a recorded pass with its old
        // values, so animating needs the view dirtied every frame. This notifies
        // the view without rebuilding the window.
        window.request_animation_frame();
    }
}

impl PanelSettings for MilkdropPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    /// On, unlike the shader panel's: a surface shader over the frame is half
    /// the reason it goes through a chain.
    fn surface_shader(&self) -> bool {
        true
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Presets", icons::FOLDER), ("Tuning", icons::SLIDERS)]
    }

    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            "Tuning" => self.tuning_page(window, cx).into_any_element(),
            _ => self.presets_page(window, cx).into_any_element(),
        }
    }
}

impl MilkdropPanel {
    fn presets_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Opening the page picks up a pack dropped in since the last look. See
        // [`RootsStamp`].
        self.rescan_if_stale();

        let roots = core_settings::milkdrop_roots();
        let mut roots_body = div().flex().flex_col().gap(tokens::SPACE_SM);
        for (index, root) in roots.iter().enumerate() {
            roots_body = roots_body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::SPACE_MD)
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .child(root.to_string_lossy().into_owned()),
                    )
                    .child(crate::settings::ui::small_button(
                        rox_i18n::t!("milkdrop-remove-root"),
                        icons::CLOSE,
                        false,
                        cx.listener(move |this, _, _, cx| this.remove_root(index, cx)),
                    )),
            );
        }
        roots_body = roots_body.child(crate::settings::ui::small_button(
            rox_i18n::t!("milkdrop-add-root"),
            icons::FOLDER_PLUS,
            false,
            cx.listener(|this, _, window, cx| this.pick_root(window, cx)),
        ));

        let folder = rox_core::settings::milkdrop_dir();
        let home = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_xs()
                            .child(folder.join("presets").to_string_lossy().into_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("milkdrop-packs-hint")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .child(crate::settings::ui::small_button(
                        rox_i18n::t!("milkdrop-open-folder"),
                        icons::FOLDER,
                        false,
                        |_, _, cx| Self::open_presets_folder(cx),
                    )),
            );

        let total = self.library().presets().len();
        let favorites: HashSet<PathBuf> = core_settings::milkdrop_favorites().into_iter().collect();
        let locked = self.config.locked;

        let mut rotations: Vec<(RotationChoice, SharedString)> = vec![
            (RotationChoice::All, rox_i18n::t!("milkdrop-rotation-all")),
            (
                RotationChoice::Favorites,
                rox_i18n::t!(
                    "milkdrop-rotation-favorites",
                    count = favorites.len().to_string()
                ),
            ),
        ];
        rotations.extend(self.folder_options.iter().cloned());
        let rotation = panel::picker(
            "milkdrop-rotation",
            self.rotation_choice(),
            rotations,
            false,
            |this: &mut Self, choice, cx| this.set_rotation(choice, cx),
            cx,
        );
        // Favorites with nothing starred is the one pick that doesn't do what
        // it says, so the page says so.
        let no_favorites = (self.config.favorites_only && favorites.is_empty())
            .then(|| rox_i18n::t!("milkdrop-no-favorites"));

        let current_path = self.current_path();
        let starred = self.current_is_favorite();
        let has_current = current_path.is_some();
        let star_target = current_path.clone();
        let showing: SharedString = current_path
            .as_deref()
            .map(preset_label)
            .map(SharedString::from)
            .unwrap_or_else(|| rox_i18n::t!("milkdrop-no-preset"));
        let now_showing = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(tokens::SPACE_XS)
            .child(crate::settings::ui::small_button(
                rox_i18n::t!(if starred {
                    "milkdrop-favorited"
                } else {
                    "milkdrop-favorite-short"
                }),
                if starred {
                    icons::STAR_FILLED
                } else {
                    icons::STAR
                },
                !has_current,
                cx.listener(move |this, _, _, cx| {
                    if let Some(path) = star_target.clone() {
                        this.toggle_favorite(path, cx);
                    }
                }),
            ))
            .child(crate::settings::ui::small_button(
                rox_i18n::t!("milkdrop-reveal-short"),
                icons::EXTERNAL_LINK,
                !has_current,
                cx.listener(|this, _, _, cx| this.reveal_preset(cx)),
            ))
            // A settings page is no place for a hundred thousand rows.
            .child(crate::settings::ui::small_button(
                rox_i18n::t!("milkdrop-choose"),
                icons::SEARCH,
                false,
                cx.listener(|_, _, _, cx| {
                    rox_panel_api::openers::milkdrop_picker(
                        Box::new(PanelHost(cx.entity().downgrade())),
                        cx,
                    );
                }),
            ));

        // 1 to 120 seconds, linear: a log strip would spend most of its travel
        // on the part nobody scrubs through.
        let seconds = self.config.duration_secs;
        let duration = panel::value_slider_edit(
            &self.duration_scrub,
            &self.value_edit,
            ((seconds as f32 - 1.0) / 119.0).clamp(0.0, 1.0),
            format!("{seconds:.0}s"),
            format!("{seconds:.0}"),
            |seconds: f32| ((seconds - 1.0) / 119.0).clamp(0.0, 1.0),
            |this: &mut Self, fraction, cx| {
                let seconds = (1.0 + fraction * 119.0) as f64;
                this.config.duration_secs = seconds;
                this.send(Command::SetPresetDuration(seconds));
                cx.notify();
            },
            cx,
        );

        let body = div().flex().flex_col().gap(tokens::SPACE_SM);
        let body = if total == 0 {
            body.child(self.empty_presets())
        } else {
            // A block: the name runs long and the buttons need the width.
            body.child(panel::setting_block(
                rox_i18n::t!("milkdrop-current"),
                Some(showing),
                None,
                now_showing,
            ))
            .child(setting_row(
                rox_i18n::t!("milkdrop-locked"),
                Some(rox_i18n::t!("milkdrop-locked.description")),
                toggle(
                    locked,
                    |this: &mut Self, on, cx| this.set_locked(on, cx),
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("milkdrop-duration"),
                Some(rox_i18n::t!("milkdrop-duration.description")),
                duration,
            ))
            .child(setting_row(
                rox_i18n::t!("milkdrop-rotation"),
                Some(rox_i18n::t!("milkdrop-rotation.description")),
                rotation,
            ))
            .children(no_favorites.map(|line| {
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(line)
            }))
        };

        let rescan = crate::settings::ui::small_button(
            rox_i18n::t!("milkdrop-rescan"),
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, _, cx| this.rescan_presets(cx)),
        )
        .into_any_element();

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(rox_i18n::t!("milkdrop-roots"), None, roots_body))
            .child(section(rox_i18n::t!("milkdrop-folder"), None, home))
            .child(section(
                rox_i18n::t!("milkdrop-presets"),
                Some(rescan),
                body,
            ))
    }

    /// The Rescan button sits in the section header above this.
    fn empty_presets(&self) -> Div {
        let mut packs = div().flex().flex_col().gap(px(2.));
        for (name, url) in PACKS {
            packs = packs.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .hover(|d| d.text_color(palette::text_bright()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| cx.open_url(url))
                    .child(name),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(div().child(rox_i18n::t!("milkdrop-empty")))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("milkdrop-empty.description")),
            )
            .child(packs)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .child(crate::settings::ui::small_button(
                        rox_i18n::t!("milkdrop-open-folder"),
                        icons::FOLDER,
                        false,
                        |_, _, cx| Self::open_presets_folder(cx),
                    )),
            )
    }

    fn tuning_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let config = self.config.clone();

        // The Appearance page's backdrop rate strip, so the two visuals tune
        // alike.
        let fps = crate::settings::ui::scalar(
            &self.fps_scrub,
            &self.value_edit,
            config.fps as f32,
            crate::settings::ui::span(FPS_MIN as f32, FPS_MAX as f32, " fps").hard(),
            |this: &mut Self, fps, cx| {
                let fps = (fps.round() as u32).clamp(FPS_MIN, FPS_MAX);
                if this.config.fps != fps {
                    this.config.fps = fps;
                    this.send(Command::SetFps(fps));
                }
                cx.notify();
            },
            cx,
        );

        let sensitivity = panel::value_slider_edit(
            &self.sensitivity_scrub,
            &self.value_edit,
            (config.beat_sensitivity / 5.0).clamp(0.0, 1.0),
            format!("{:.2}", config.beat_sensitivity),
            format!("{:.2}", config.beat_sensitivity),
            |value: f32| (value / 5.0).clamp(0.0, 1.0),
            |this: &mut Self, fraction, cx| {
                let value = fraction * 5.0;
                this.config.beat_sensitivity = value;
                this.send(Command::SetBeatSensitivity(value));
                cx.notify();
            },
            cx,
        );

        // The paint closure commits a new scale only after measuring the size
        // twice running, so a drag across the strip reallocates once, when it
        // stops.
        let percent = (config.scale * 100.0).round();
        let scale = panel::value_slider_edit(
            &self.scale_scrub,
            &self.value_edit,
            ((config.scale - SCALE_MIN) / (SCALE_MAX - SCALE_MIN)).clamp(0.0, 1.0),
            format!("{percent:.0}%"),
            format!("{percent:.0}"),
            |percent: f32| {
                ((percent / 100.0 - SCALE_MIN) / (SCALE_MAX - SCALE_MIN)).clamp(0.0, 1.0)
            },
            |this: &mut Self, fraction, cx| {
                // Snapped to whole percent so the readout holds still under a drag and
                // a typed value comes back unchanged.
                let percent = (SCALE_MIN + fraction * (SCALE_MAX - SCALE_MIN)) * 100.0;
                this.config.scale = percent.round() / 100.0;
                cx.notify();
            },
            cx,
        );

        let tint_percent = (config.tint_strength * 100.0).round();
        let tint = panel::value_slider_edit(
            &self.tint_scrub,
            &self.value_edit,
            config.tint_strength.clamp(0.0, 1.0),
            format!("{tint_percent:.0}%"),
            format!("{tint_percent:.0}"),
            |percent: f32| (percent / 100.0).clamp(0.0, 1.0),
            |this: &mut Self, fraction, cx| {
                this.config.tint_strength = (fraction * 100.0).round() / 100.0;
                cx.notify();
            },
            cx,
        );

        let color = panel::choices_shared(
            &[
                (rox_i18n::t!("milkdrop-color-preset"), MilkdropColor::Preset),
                (rox_i18n::t!("milkdrop-color-theme"), MilkdropColor::Theme),
                (
                    rox_i18n::t!("milkdrop-color-palette"),
                    MilkdropColor::Palette,
                ),
                (rox_i18n::t!("milkdrop-color-cover"), MilkdropColor::Cover),
            ],
            config.color,
            |this: &mut Self, color, cx| {
                this.config.color = color;
                cx.notify();
            },
            cx,
        );

        let idle = panel::choices_shared(
            &[
                (rox_i18n::t!("milkdrop-idle-hold"), false),
                (rox_i18n::t!("milkdrop-idle-fade"), true),
            ],
            config.fade,
            |this: &mut Self, fade, cx| {
                this.config.fade = fade;
                cx.notify();
            },
            cx,
        );

        // Snapped to a tenth of a second, for the same reason as the scale.
        let fade = panel::value_slider_edit(
            &self.fade_scrub,
            &self.value_edit,
            (config.fade_secs / FADE_MAX).clamp(0.0, 1.0),
            format!("{:.1}s", config.fade_secs),
            format!("{:.1}", config.fade_secs),
            |seconds: f32| (seconds / FADE_MAX).clamp(0.0, 1.0),
            |this: &mut Self, fraction, cx| {
                this.config.fade_secs = (fraction * FADE_MAX * 10.0).round() / 10.0;
                cx.notify();
            },
            cx,
        );

        div().flex().flex_col().gap(SECTION_GAP).child(section(
            rox_i18n::t!("milkdrop-tuning"),
            None,
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(setting_row(
                    rox_i18n::t!("milkdrop-beat-sensitivity"),
                    Some(rox_i18n::t!("milkdrop-beat-sensitivity.description")),
                    sensitivity,
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-hard-cuts"),
                    Some(rox_i18n::t!("milkdrop-hard-cuts.description")),
                    toggle(
                        config.hard_cuts,
                        |this: &mut Self, on, cx| {
                            this.config.hard_cuts = on;
                            this.send(Command::SetHardCut(on));
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-fps"),
                    Some(rox_i18n::t!("milkdrop-fps.description")),
                    fps,
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-scale"),
                    Some(rox_i18n::t!("milkdrop-scale.description")),
                    scale,
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-color"),
                    Some(rox_i18n::t!("milkdrop-color.description")),
                    color,
                ))
                // Palette and Cover replace the preset's hues, so the tint slider only
                // shows where it does something.
                .when(
                    matches!(config.color, MilkdropColor::Preset | MilkdropColor::Theme),
                    |page| {
                        page.child(setting_row(
                            rox_i18n::t!("milkdrop-tint"),
                            Some(rox_i18n::t!("milkdrop-tint.description")),
                            tint,
                        ))
                    },
                )
                .child(setting_row(
                    rox_i18n::t!("milkdrop-idle"),
                    Some(rox_i18n::t!("milkdrop-idle.description")),
                    idle,
                ))
                // Each shows only where it does something: the duration with a fade,
                // focus on a held frame. Dimmed in place, either would still move.
                .when(config.fade, |page| {
                    page.child(setting_row(
                        rox_i18n::t!("milkdrop-fade-duration"),
                        None,
                        fade,
                    ))
                })
                .when(!config.fade, |page| {
                    page.child(setting_row(
                        rox_i18n::t!("milkdrop-run-focused"),
                        Some(rox_i18n::t!("milkdrop-run-focused.description")),
                        toggle(
                            config.run_focused,
                            |this: &mut Self, on, cx| {
                                this.config.run_focused = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                })
                .child(setting_row(
                    rox_i18n::t!("milkdrop-show-name"),
                    Some(rox_i18n::t!("milkdrop-show-name.description")),
                    toggle(
                        config.show_preset_name,
                        |this: &mut Self, on, cx| {
                            this.config.show_preset_name = on;
                            if !on {
                                this.banner = None;
                            }
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .when(config.show_preset_name, |page| {
                    page.child(setting_row(
                        rox_i18n::t!("milkdrop-name-always"),
                        Some(rox_i18n::t!("milkdrop-name-always.description")),
                        toggle(
                            config.name_always,
                            |this: &mut Self, on, cx| {
                                this.config.name_always = on;
                                // Switched on between changes, the name has to come up now.
                                if on
                                    && this.banner.is_none()
                                    && let Some(path) = this.current_path()
                                {
                                    this.banner = Some((preset_label(&path), Instant::now()));
                                }
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                })
                .child(setting_row(
                    rox_i18n::t!("milkdrop-show-controls"),
                    Some(rox_i18n::t!("milkdrop-show-controls.description")),
                    toggle(
                        config.show_controls,
                        |this: &mut Self, on, cx| {
                            this.config.show_controls = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-flip-horizontal"),
                    Some(rox_i18n::t!("milkdrop-flip-horizontal.description")),
                    toggle(
                        config.flip_horizontal,
                        |this: &mut Self, on, cx| {
                            this.config.flip_horizontal = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(setting_row(
                    rox_i18n::t!("milkdrop-flip-vertical"),
                    Some(rox_i18n::t!("milkdrop-flip-vertical.description")),
                    toggle(
                        config.flip_vertical,
                        |this: &mut Self, on, cx| {
                            this.config.flip_vertical = on;
                            cx.notify();
                        },
                        cx,
                    ),
                )),
        ))
    }
}

impl EventEmitter<PanelEvent> for MilkdropPanel {}

impl Focusable for MilkdropPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MilkdropPanel {
    fn panel_name(&self) -> &'static str {
        "milkdrop"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-milkdrop"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let next = panel.downgrade();
        let previous = panel.downgrade();
        let random = panel.downgrade();
        let choose = panel.downgrade();
        let reveal = panel.downgrade();
        let folder = rox_core::settings::milkdrop_dir().join("presets");

        let menu = menu
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-next"))
                    .icon(Icon::default().path(icons::SKIP_FORWARD))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = next.upgrade() {
                            panel.read(cx).send(Command::NextPreset { smooth: true });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-previous"))
                    .icon(Icon::default().path(icons::SKIP_BACK))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = previous.upgrade() {
                            panel
                                .read(cx)
                                .send(Command::PreviousPreset { smooth: true });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-random"))
                    .icon(Icon::default().path(icons::SHUFFLE))
                    .on_click(move |_, _, cx| {
                        if let Some(panel) = random.upgrade() {
                            panel.read(cx).send(Command::NextPreset { smooth: false });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-choose"))
                    .icon(Icon::default().path(icons::SEARCH))
                    .on_click(move |_, _, cx| {
                        rox_panel_api::openers::milkdrop_picker(
                            Box::new(PanelHost(choose.clone())),
                            cx,
                        );
                    }),
            )
            .item(panel::check_row(
                rox_i18n::t!("milkdrop-locked"),
                Some(icons::LOCK),
                |this: &Self| this.config.locked,
                |this: &mut Self, cx| {
                    let locked = !this.config.locked;
                    this.set_locked(locked, cx);
                },
                &panel,
            ))
            .separator()
            .item(panel::check_row(
                rox_i18n::t!("milkdrop-favorite"),
                Some(icons::STAR),
                |this: &Self| this.current_is_favorite(),
                |this: &mut Self, cx| {
                    if let Some(path) = this.current_path() {
                        this.toggle_favorite(path, cx);
                    }
                },
                &panel,
            ))
            .item(panel::check_row(
                rox_i18n::t!("milkdrop-favorites-only"),
                Some(icons::STAR_FILLED),
                |this: &Self| this.config.favorites_only,
                |this: &mut Self, cx| {
                    let on = !this.config.favorites_only;
                    this.set_favorites_only(on, cx);
                },
                &panel,
            ))
            .separator()
            .item(panel::check_row(
                rox_i18n::t!("milkdrop-flip-horizontal"),
                Some(icons::FLIP_HORIZONTAL),
                |this: &Self| this.config.flip_horizontal,
                |this: &mut Self, _| this.config.flip_horizontal = !this.config.flip_horizontal,
                &panel,
            ))
            .item(panel::check_row(
                rox_i18n::t!("milkdrop-flip-vertical"),
                Some(icons::FLIP_VERTICAL),
                |this: &Self| this.config.flip_vertical,
                |this: &mut Self, _| this.config.flip_vertical = !this.config.flip_vertical,
                &panel,
            ))
            .separator()
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-reveal"))
                    .icon(Icon::default().path(icons::EXTERNAL_LINK))
                    .disabled(self.current.is_none())
                    .on_click(move |_, _, cx| {
                        let path = reveal
                            .upgrade()
                            .and_then(|panel| panel.read(cx).current_path());
                        if let Some(path) = path {
                            cx.reveal_path(&path);
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("milkdrop-open-folder"))
                    .icon(Icon::default().path(icons::FOLDER))
                    .on_click(move |_, _, cx| {
                        // Nothing else creates the folder, and the platform refuses a missing
                        // path.
                        std::fs::create_dir_all(&folder).ok();
                        cx.open_with_system(&folder);
                    }),
            );

        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                MilkdropPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for MilkdropPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl MilkdropPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self._focus_wake.is_none() {
            let panel = cx.entity_id();
            self._focus_wake = Some(window.on_focus_in(&self.focus.clone(), cx, move |_, cx| {
                cx.notify(panel);
            }));
        }
        // `Some` means a paint has run with a real size: the cue to start the
        // worker.
        let size = self.draw.lock().unwrap().size;
        if let (Some(size), None) = (size, self.engine.as_ref())
            && !self.quitting
        {
            self.start(size, cx);
        }
        self.drain_events();
        self.follow_lists();
        self.park_or_resume(window, cx);
        let tinting = self.step_tint();

        // Starting counts: the first render lands a few frames after the worker
        // goes up. A fade counts too: its own frames carry the opacity to zero
        // and park the worker.
        let fading = self.fading();
        let animate = fading
            || tinting
            || (!self.parked
                && matches!(
                    self.engine.as_ref().map(Engine::status),
                    Some(Status::Starting) | Some(Status::Running { .. })
                ));

        // The fade happens in the pass, blending toward what the body painted,
        // so dropping the canvas at zero changes nothing on screen. See
        // [`FRAME_WGSL`].
        let opacity = self.opacity();
        let show = opacity > 0.0 || fading;
        // Read inside the themed body, so a panel with its own theme grades
        // against that theme's background.
        let cover = palette::seed(self.state.player.entity_id()).and_then(|seed| seed.primary);
        let grade = Grade::from_scope(grade_mode(self.config.color), cover);
        let look = Look {
            fade: fade::mix(opacity),
            hue: self.tint.hue,
            tint: if grade.tints() {
                self.tint.amount * self.config.tint_strength.clamp(0.0, 1.0)
            } else {
                0.0
            },
            grade,
            flip_horizontal: self.config.flip_horizontal,
            flip_vertical: self.config.flip_vertical,
        };

        let banner = self
            .banner
            .as_ref()
            .filter(|(_, at)| self.config.name_always || at.elapsed() < BANNER_HOLD)
            .map(|(text, _)| text.clone());
        let error = self.error();
        let controls = self.config.show_controls && error.is_none();
        // The controls stay up under a stall: pressing Next and seeing a name
        // land tells a live worker from a dead one.
        let overlay = match error {
            Some(message) => Some((rox_i18n::t!("milkdrop-no-gl"), SharedString::from(message))),
            None => self.stall(),
        };
        let starred = self.current_is_favorite();

        let scale = self.config.scale;
        let engine = self.engine.clone();
        let draw = self.draw.clone();
        let panel = cx.entity().entity_id();

        div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            // The frame helper wants a plain div at the root, so the body
            // that needs an id sits one level in.
            .child(
                div()
                    .id("milkdrop-body")
                    .size_full()
                    .relative()
                    // Hover is tracked here rather than through a group: the strip is
                    // deferred, and a group's hitbox only registers while its own subtree
                    // paints.
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if this.hovered != *hovered {
                            this.hovered = *hovered;
                            cx.notify();
                        }
                    }))
                    .when(show, |body| {
                        body.child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, cx| {
                                    paint(
                                        bounds,
                                        window,
                                        cx,
                                        scale,
                                        look,
                                        animate,
                                        engine.as_ref(),
                                        &draw,
                                        panel,
                                    );
                                },
                            )
                            .size_full(),
                        )
                    })
                    // Everything over the visual is deferred. DirectX runs shader regions
                    // once, at the deferred-draw boundary, after every ordinary primitive,
                    // so anything painted in tree order sits under the opaque frame. Blade
                    // runs regions in paint order.
                    .children(banner.map(|text| {
                        deferred(
                            div()
                                .absolute()
                                .left(tokens::SPACE_MD)
                                .bottom(tokens::SPACE_MD)
                                .px(tokens::SPACE_SM)
                                .py(px(2.))
                                .rounded(tokens::RADIUS)
                                .bg(palette::bg_control())
                                .text_xs()
                                .text_color(palette::text())
                                .child(text),
                        )
                    }))
                    .when(controls, |body| {
                        body.child(deferred(self.controls(starred, cx)))
                    })
                    .children(overlay.map(|(headline, detail)| {
                        deferred(
                            div()
                                .absolute()
                                .inset_0()
                                .p(tokens::SPACE_MD)
                                .flex()
                                .flex_col()
                                .gap(tokens::SPACE_SM)
                                .items_center()
                                .justify_center()
                                .text_center()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(headline)
                                .child(detail),
                        )
                    })),
            )
    }
}

impl MilkdropPanel {
    /// Invisible until the pointer is over the panel; the buttons are still
    /// there under the zero opacity.
    fn controls(&self, starred: bool, cx: &mut Context<Self>) -> Div {
        use crate::settings::ui::icon_button;
        div()
            .absolute()
            .right(tokens::SPACE_MD)
            .bottom(tokens::SPACE_MD)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .opacity(if self.hovered { 1. } else { 0. })
            .child(icon_button(
                icons::SKIP_BACK,
                false,
                cx.listener(|this, _, _, _| this.send(Command::PreviousPreset { smooth: true })),
            ))
            .child(icon_button(
                icons::SKIP_FORWARD,
                false,
                cx.listener(|this, _, _, _| this.send(Command::NextPreset { smooth: true })),
            ))
            .child(icon_button(
                icons::SHUFFLE,
                false,
                cx.listener(|this, _, _, _| this.send(Command::NextPreset { smooth: false })),
            ))
            .child(
                icon_button(
                    if starred {
                        icons::STAR_FILLED
                    } else {
                        icons::STAR
                    },
                    self.current.is_none(),
                    cx.listener(|this, _, _, cx| {
                        if let Some(path) = this.current_path() {
                            this.toggle_favorite(path, cx);
                        }
                    }),
                )
                .when(starred, |star| star.text_color(palette::accent())),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuned() -> MilkdropConfig {
        MilkdropConfig {
            chrome: PanelChrome {
                title: Some("Wall".to_string()),
                locked: true,
                ..PanelChrome::default()
            },
            preset: Some("Geiss - Spiral Artifact.milk".to_string()),
            flip_horizontal: true,
            flip_vertical: false,
            locked: true,
            duration_secs: 12.0,
            beat_sensitivity: 2.5,
            hard_cuts: false,
            fps: 30,
            scale: 0.75,
            show_preset_name: false,
            name_always: true,
            show_controls: false,
            fade: true,
            fade_secs: 2.5,
            run_focused: true,
            tint_strength: 0.6,
            rotation_folder: Some("cream/Fractal".to_string()),
            favorites_only: true,
            color: MilkdropColor::Palette,
        }
    }

    #[test]
    fn config_round_trips_through_a_dump() {
        let config = tuned();
        let dumped = serde_json::to_value(config.clone()).expect("dump");
        assert_eq!(dumped["locked"], true, "the dock pin keeps the shared key");
        assert_eq!(dumped["preset_locked"], true, "the preset lock is its own");
        // The colour mode is a word on the wire, so a hand edit reads.
        assert_eq!(dumped["color"], "palette");
        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");

        assert_eq!(read.chrome.title.as_deref(), Some("Wall"));
        assert!(read.chrome.locked);
        assert_eq!(read.preset, config.preset);
        assert!(read.flip_horizontal);
        assert!(!read.flip_vertical);
        // The rename keeps the preset lock off the dock pin's `locked` key.
        assert!(read.locked);
        assert_eq!(read.duration_secs, 12.0);
        assert_eq!(read.beat_sensitivity, 2.5);
        assert!(!read.hard_cuts);
        assert_eq!(read.fps, 30);
        assert_eq!(read.scale, 0.75);
        assert!(!read.show_preset_name);
        assert!(read.name_always);
        assert!(!read.show_controls);
        assert!(read.fade);
        assert_eq!(read.fade_secs, 2.5);
        assert!(read.run_focused);
        assert_eq!(read.tint_strength, 0.6);
        assert_eq!(read.rotation_folder, config.rotation_folder);
        assert!(read.favorites_only);
        assert_eq!(read.color, MilkdropColor::Palette);
    }

    #[test]
    fn a_dump_without_favorites_or_a_color_reads_as_before() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("favorites_only").expect("written");
        object.remove("color").expect("written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert!(!read.favorites_only);
        assert_eq!(read.color, MilkdropColor::Theme);
        assert_eq!(read.rotation_folder, tuned().rotation_folder);
    }
    #[test]
    fn a_hue_turns_the_short_way_round() {
        use std::f32::consts::{PI, TAU};

        assert_eq!(turned_hue(1.0, 2.5, 0.0), 1.0);
        assert!((turned_hue(1.0, 2.5, 1.0) - 2.5).abs() < 1e-5);
        assert!((turned_hue(1.0, 2.0, 0.5) - 1.5).abs() < 1e-5);

        // Across the wrap the turn goes forward by a hair, not back through
        // every hue.
        let turned = turned_hue(TAU - 0.1, 0.1, 1.0);
        assert!(turned > TAU - 0.1, "went the long way: {turned}");
        assert!((turned - (TAU + 0.1)).abs() < 1e-5);

        let back = turned_hue(0.1, TAU - 0.1, 1.0);
        assert!(back < 0.1, "went the long way: {back}");

        // An exact half turn has no short way: either arc will do, as long as
        // it moves half a turn and stays finite.
        let opposite = turned_hue(0.0, PI, 1.0);
        assert!((opposite.abs() - PI).abs() < 1e-5, "moved {opposite}");

        assert!((turned_hue(1.0, 2.0, 5.0) - 2.0).abs() < 1e-5);
        assert!((turned_hue(1.0, 2.0, -5.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_tint_eases_in_across_and_out() {
        use std::f32::consts::PI;

        // From nothing, the first step takes the hue whole and ramps the amount.
        let first = ease_tint(TintAim::default(), Some(2.0), 0.25);
        assert_eq!(first.hue, 2.0);
        assert_eq!(first.amount, 0.25);

        let showing = TintAim {
            hue: 0.0,
            amount: 1.0,
        };
        let moved = ease_tint(showing, Some(1.0), 0.1);
        assert!(
            (moved.hue - 0.1 * PI).abs() < 1e-5,
            "a tenth of the ease is a tenth of a half turn: {}",
            moved.hue
        );
        assert_eq!(moved.amount, 1.0, "a full tint has nowhere left to ramp");

        let landed = ease_tint(showing, Some(1.0), 1.0);
        assert_eq!(landed.hue, 1.0);

        // Letting go holds the hue, so the last of the tint stays its colour.
        let letting_go = ease_tint(showing, None, 0.25);
        assert_eq!(letting_go.hue, 0.0);
        assert_eq!(letting_go.amount, 0.75);

        assert_eq!(ease_tint(showing, None, 10.0).amount, 0.0);
        assert_eq!(ease_tint(showing, Some(0.0), 10.0).amount, 1.0);
        assert_eq!(ease_tint(TintAim::default(), None, 0.5).amount, 0.0);
    }

    #[test]
    fn a_dump_without_a_fade_reads_as_the_default() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fade_secs").expect("the fade was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.fade_secs, 1.0);
        assert_eq!(read.scale, 0.75);
    }

    #[test]
    fn a_dump_without_the_fade_switch_keeps_its_duration() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fade").expect("the switch was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert!(!read.fade);
        assert_eq!(read.fade_secs, 2.5);
    }

    #[test]
    fn a_dump_without_a_tint_reads_as_the_default() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object
            .remove("tint_strength")
            .expect("the tint was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.tint_strength, TINT_DEFAULT);
    }

    #[test]
    fn a_dump_without_a_field_reads_as_the_default() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fps").expect("fps was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.fps, 60);
        assert_eq!(read.duration_secs, 12.0);
    }

    #[test]
    fn no_preset_writes_no_key() {
        let dumped = serde_json::to_value(MilkdropConfig::default()).expect("dump");
        assert!(dumped.get("preset").is_none());
        assert!(dumped.get("rotation_folder").is_none());
    }

    /// Relative to the root: the absolute path is mostly the part every
    /// option shares.
    #[test]
    fn a_rotation_folder_is_named_under_its_root() {
        let roots = vec![
            PathBuf::from("/home/me/.local/share/rox/milkdrop/presets"),
            PathBuf::from("/mnt/packs"),
        ];
        assert_eq!(
            folder_label(
                Path::new("/home/me/.local/share/rox/milkdrop/presets/cream/Fractal"),
                &roots,
            ),
            "cream/Fractal"
        );
        assert_eq!(
            folder_label(Path::new("/mnt/packs/Dancer"), &roots),
            "Dancer"
        );
        // Under no root, the whole path is the only honest answer.
        assert_eq!(
            folder_label(Path::new("/elsewhere/Fractal"), &roots),
            "/elsewhere/Fractal"
        );
        // A root itself would strip to a blank row.
        assert_eq!(folder_label(Path::new("/mnt/packs"), &roots), "/mnt/packs");
    }

    #[test]
    fn the_config_round_trips_through_a_panel_state() {
        use rox_dock::{PanelInfo, PanelState};

        let dump = serde_json::to_value(PanelState {
            panel_name: "StackPanel".to_string(),
            children: vec![PanelState {
                panel_name: "milkdrop".to_string(),
                children: Vec::new(),
                info: PanelInfo::panel(serde_json::to_value(tuned()).expect("dump")),
            }],
            info: PanelInfo::stack(Vec::new(), gpui::Axis::Vertical),
        })
        .expect("dump the dock state");

        let read: PanelState = serde_json::from_value(dump).expect("read the dock state");
        let PanelInfo::Panel(config) = &read.children[0].info else {
            panic!("the milkdrop panel's config should still be a panel dump");
        };
        let config: MilkdropConfig = serde_json::from_value(config.clone()).expect("read");
        assert_eq!(
            config.preset.as_deref(),
            Some("Geiss - Spiral Artifact.milk")
        );
        assert_eq!(config.fps, 30);
        assert!(config.locked);
    }

    /// The pass is a string constant, so nothing but a running window would
    /// compile it otherwise.
    #[test]
    fn the_frame_pass_compiles_with_the_grade_in_scope() {
        crate::panel::shader::validate_frame_pass(&grade::wgsl(FRAME_WGSL), &["frame"])
            .expect("the Milkdrop panel's pass validates");
    }

    #[test]
    fn the_render_size_clamps_both_sides() {
        assert_eq!(render_size(800.0, 450.0, 2.0, 1.0), (1600, 900));
        assert_eq!(render_size(800.0, 400.0, 1.0, 0.5), (400, 200));
        assert_eq!(render_size(4000.0, 3.0, 1.0, 1.0), (4000, MIN_SIDE));
        assert_eq!(render_size(6000.0, 6000.0, 2.0, 1.0), (MAX_SIDE, MAX_SIDE));
        assert_eq!(render_size(0.0, -10.0, 1.0, 1.0), (MIN_SIDE, MIN_SIDE));
    }

    /// Forward slashes, and the shortest form when roots overlap.
    #[test]
    fn a_preset_is_carried_as_its_path_under_the_root() {
        let roots = vec![
            PathBuf::from("/home/me/rox/milkdrop/presets"),
            PathBuf::from("/mnt"),
        ];
        assert_eq!(
            relative_to_roots(
                Path::new("/home/me/rox/milkdrop/presets/cream/Fractal/Geiss.milk"),
                &roots
            )
            .as_deref(),
            Some("cream/Fractal/Geiss.milk")
        );
        assert_eq!(
            relative_to_roots(Path::new("/elsewhere/a.milk"), &roots),
            None
        );
    }

    #[test]
    fn a_preset_is_kept_by_name_and_an_old_path_reads_as_its_name() {
        assert_eq!(preset_name("Geiss - Spiral.milk"), "Geiss - Spiral.milk");
        assert_eq!(
            preset_name("/home/me/packs/cream/Fractal/Geiss - Spiral.milk"),
            "Geiss - Spiral.milk"
        );
        let dumped = serde_json::to_value(tuned()).expect("dump");
        assert!(!dumped["preset"].as_str().unwrap().contains('/'));
        assert!(!dumped["rotation_folder"].as_str().unwrap().starts_with('/'));
    }

    #[test]
    fn a_saved_preset_is_found_again_by_name() {
        let presets = vec![
            PathBuf::from("/Users/them/packs/cream/Fractal/Geiss.milk"),
            PathBuf::from("/Users/them/packs/other/Rovastar.milk"),
        ];
        assert_eq!(
            find_by_name(&presets, "Rovastar.milk"),
            Some(PathBuf::from("/Users/them/packs/other/Rovastar.milk"))
        );
        assert_eq!(find_by_name(&presets, "Nope.milk"), None);
    }

    #[test]
    fn a_saved_rotation_folder_is_found_again_under_another_root() {
        let roots = vec![PathBuf::from("/Users/them/packs")];
        let folders = vec![
            PathBuf::from("/Users/them/packs/cream/Fractal"),
            PathBuf::from("/Users/them/packs/cream/Dancer"),
            PathBuf::from("/Users/them/packs/other/Waveform"),
        ];
        assert_eq!(
            find_folder_by_relative(&folders, &roots, "cream/Dancer"),
            Some(PathBuf::from("/Users/them/packs/cream/Dancer"))
        );
        assert_eq!(
            find_folder_by_relative(&folders, &roots, "original/Waveform"),
            Some(PathBuf::from("/Users/them/packs/other/Waveform"))
        );
        assert_eq!(
            find_folder_by_relative(&folders, &roots, "cream/Nope"),
            None
        );
    }
}
