//! The Milkdrop panel: twenty years of MilkDrop presets, rendered by
//! libprojectM on a thread of its own and handed to gpui a frame at a time.
//!
//! Nothing here draws a visual. [`rox_milkdrop::Engine`] owns a private
//! OpenGL context, renders into its own framebuffer, reads the pixels back
//! and publishes an rgba8 buffer; this file's whole job is to get that
//! buffer onto the screen and to give the user the knobs projectM takes.
//! The panel is the boundary where a foreign renderer becomes an ordinary
//! rox panel that docks, tabs, pops out and takes a theme.
//!
//! ## Why a shader chain and not an image
//!
//! Each frame goes up as a dynamic user texture and is drawn through a
//! one-pass chain that samples it. The obvious alternative, an `img()` with
//! a fresh `RenderImage` every frame, runs through the sprite atlas, which
//! means an allocation and a drop per frame for something that is really a
//! video stream. The chain path also means the frame composes like every
//! other shader surface in the app, so a Milkdrop panel can wear a surface
//! shader over the top like anything else.
//!
//! The texture and the chain are keyed by the window that handed them out,
//! since a compiled pipeline and an uploaded texture both live in one
//! window's renderer. Popping the panel out registers a fresh pair in the
//! new window.
//!
//! ADR 28 is the decision this sits under, including the readback cost it
//! takes on purpose.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    canvas, deferred, div, prelude::*, px, AnyElement, App, Context, Div, EntityId, EventEmitter,
    FocusHandle, Focusable, MouseButton, PathPromptOptions, SharedString, Subscription,
    UserShaderChain, UserShaderId, UserShaderPass, UserTextureId, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_core::settings::{self as core_settings, MilkdropColor};
use rox_milkdrop::{Command, Engine, EngineOptions, Event, PresetLibrary, Rotation, Status};
use rox_panel_api::preset_browser::{
    find_folder_by_relative, folder_label, preset_label, relative_to_roots, PresetHost, Thumb,
    Thumbnails,
};
use rox_panel_kit::fade::{self, Fade};
use rox_panel_kit::grade::{self, Grade, GradeMode};

use crate::assets::icons;
use crate::design::{palette, tokens};
// The surface-shader module, for the eight meta floats every pass gets in
// scope. Aliased the way `panels::shader` aliases it, since that file is
// `panel::shader` and one letter apart from this crate's own `shader`.
use crate::panel::shader as surface;
use crate::panel::{
    self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit,
};
use crate::panel_settings;
use crate::settings::ui::{section, SECTION_GAP};

/// The smallest and largest render each side gets, whatever the panel's
/// size and scale multiply out to. The floor keeps a panel dragged down to
/// a sliver from asking projectM for a 4x1 framebuffer; the ceiling keeps a
/// maximised panel on a 5K display from turning the readback into a
/// 30 MB-per-frame memcpy.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 4096;

/// How long a preset's name stays on screen after a switch.
const BANNER_HOLD: Duration = Duration::from_secs(2);

/// How long the panel waits on the worker before saying something. A cold
/// driver can take a second to hand over a context and the first frame is
/// two readbacks behind that, so this is well clear of a slow start and
/// well short of somebody deciding the feature is broken.
const STALL_GRACE: Duration = Duration::from_secs(5);

/// The longest fade the slider offers. Past a few seconds the panel reads
/// as broken rather than calm: the audio has been gone for ages and the
/// visual is still limping down.
const FADE_MAX: f32 = 5.0;

/// How far the tint turns hues by default. Enough that a red cover reads
/// as red across the frame and a blue one as blue, short of the point
/// where every preset comes out the same colour and the rotation reads as
/// a filter over the top.
const TINT_DEFAULT: f32 = 0.35;

/// How long the tint takes to ramp all the way in or out, and to travel
/// the longest way round the hue circle, which is half a turn. A nearer
/// cover takes proportionally less. Slower than the fade on purpose:
/// colour creeping across is the effect, and a hue that snaps on a track
/// change reads as a glitch.
const TINT_EASE: Duration = Duration::from_millis(1200);

/// What the Presets page checks before it trusts the last scan: each
/// root's own modification time. A pack dropped into a root moves it; a
/// file added deep inside a pack doesn't, and that's what the Rescan
/// button is for. A walk of the whole library is a stat per preset, and
/// the big pack runs to a hundred and fifty thousand, so it can't be a
/// thing the page does on a timer.
type RootsStamp = Vec<(PathBuf, Option<std::time::SystemTime>)>;

/// The frame rates the input will take. Below the floor the visual stops
/// reading as motion, and above the ceiling the readback is the bottleneck
/// no matter what projectM does.
const FPS_MIN: u32 = 10;
const FPS_MAX: u32 = 240;

/// The render-scale slider's span, as a fraction of the panel's device
/// pixels. A quarter is where the upscale starts looking like a mistake
/// rather than a trade.
const SCALE_MIN: f32 = 0.25;
const SCALE_MAX: f32 = 1.0;

/// The preset packs worth having, and where they live. Named rather than
/// translated: they're repository names, and typing anything else into
/// GitHub finds nothing.
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

/// The one pass that puts a frame on screen: sample the texture, aspect-fit
/// it into the panel's bounds, letterbox the rest, and carry the fade.
///
/// The fit only does anything while a resize is in flight. The texture is
/// made at the panel's own device size, so the two agree on every settled
/// frame; during the one-frame resize debounce they don't, and stretching
/// the old frame to the new shape is the ugliest possible answer.
///
/// The fade rides `signals[0].x` and comes out as premultiplied alpha,
/// which is the contract every user shader writes under. The region's
/// blend is `src + dst * (1 - src.a)`, so a fade of `f` lands the panel
/// exactly `f` of the way from whatever the body already painted to the
/// frame, and a fade of zero is the identity: the shader writes nothing
/// and the body's own `bg_root()` is what's on screen, at whatever alpha
/// the theme's surface opacity gave it. That's why there's no background
/// colour to pass in. Mixing toward a colour would have to guess how the
/// body composited over the window backdrop; blending toward the
/// destination doesn't guess.
///
/// ## The album tint and the grade
///
/// `signals[0].y` is the hue the cover's dominant colour sits at and
/// `signals[0].z` is how far the frame turns toward it, both worked out on
/// the CPU by [`MilkdropPanel::step_tint`]. The slots after them are the
/// theme grade's, filled by [`Grade::write`]; the maths for both lives in
/// [`rox_panel_kit::grade`], shared with the backdrop, and this pass just
/// calls it.
///
/// The tint is a hue rotation in Oklab, not a mix toward the cover colour.
/// Presets are already strongly coloured, and lerping every pixel toward
/// one colour flattens the palette a preset spent its whole design on:
/// the more it tints, the more of the preset it deletes. A rotation keeps
/// each pixel's lightness and chroma exactly and only turns its hue, so a
/// preset that runs green-through-cyan against a red cover comes out
/// running red-through-orange with its own shape, contrast and vividness
/// intact. Grey pixels have no hue to turn and fall out of it on their
/// own.
///
/// The maths runs on the values the sampler hands back, which are
/// linear-light: the frame is registered as an `Rgba8UnormSrgb` texture,
/// so the hardware decodes on read, and linear sRGB is the space Oklab is
/// defined from. The result goes back out the way it came in.
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

/// The Milkdrop panel's per-view config: what a saved layout restores and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MilkdropConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The preset that was on screen when the layout was saved, by file
    /// name and nothing else: `Geiss - Spiral Artifact.milk`. Restored by
    /// looking the name up in the scan, so a workspace carries the pick
    /// to another machine without carrying a path off this one. Preset
    /// names are distinctive enough that a name is the identity. A layout
    /// from before this held the whole path; the name is taken off it on
    /// load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Mirror the frame left to right.
    pub flip_horizontal: bool,
    /// Mirror the frame top to bottom.
    pub flip_vertical: bool,
    /// Stay on the current preset: no timed switch, no beat-driven cut.
    ///
    /// Renamed on the wire because [`PanelChrome`] is flattened over this
    /// struct and its own `locked` is the dock pin. Two fields can't share
    /// one JSON key, and the pin is the one every other panel writes.
    #[serde(rename = "preset_locked")]
    pub locked: bool,
    /// Seconds a preset holds before the next one comes up.
    pub duration_secs: f64,
    /// How readily projectM calls something a beat. Presets react to this,
    /// and a quiet mix wants it higher than a loud one.
    pub beat_sensitivity: f32,
    /// Let a loud enough beat cut straight to the next preset instead of
    /// waiting out the duration.
    pub hard_cuts: bool,
    /// The worker's frame rate. 30 halves the readback bill on a laptop.
    pub fps: u32,
    /// Render size as a fraction of the panel's device pixels. Below 1.0
    /// the frame is upscaled on the way in, which costs sharpness and
    /// saves the readback proportionally.
    pub scale: f32,
    /// Name the preset on screen for a couple of seconds when it changes.
    pub show_preset_name: bool,
    /// Keep the name up the whole time rather than for a couple of
    /// seconds. Only means anything with `show_preset_name` on.
    pub name_always: bool,
    /// Previous, Next, Random and the favorite star over the visual
    /// while the pointer is on the panel, so the menu isn't the only way
    /// to move.
    pub show_controls: bool,
    /// What losing the audio does to the picture. Off is hold: a pause
    /// and a stop both freeze the frame where it was, and play picks
    /// straight back up from there. On is fade: the visual goes down to
    /// the panel background over `fade_secs` and comes back the same way.
    /// Two fields rather than one, so switching the fade off and back on
    /// returns the duration that was picked instead of making the user
    /// find it again.
    pub fade: bool,
    /// Seconds the visual takes to fade to the panel background when the
    /// audio pauses or stops, and to come back when it starts again. Zero
    /// cuts straight to the background with no fade at all. Only read
    /// with `fade` on.
    pub fade_secs: f32,
    /// Keep the worker running while the panel has focus, whatever the
    /// audio is doing. Only offered with `fade` off, since a held frame is
    /// what leaves somebody sitting on the panel picking presets in
    /// silence with nothing moving. Off by default because focus is
    /// sticky: the dock hands it to the active tab and any click on the
    /// panel takes it, so with this always on a paused track under a
    /// focused panel kept cycling for people who never meant it to.
    pub run_focused: bool,
    /// How far the frame's hues turn toward the playing track's cover, 0
    /// for none and 1 for every hue landing on the cover's. On by default
    /// at a strength that reads as the visual agreeing with the album
    /// rather than wearing it.
    pub tint_strength: f32,
    /// Restrict the rotation to one folder, as its path under the scan
    /// root with forward slashes: `cream/Fractal`. `None` walks the whole
    /// library. An explicit pick from the list still loads whatever it
    /// points at; this only bounds what Next, Previous and the timed
    /// switch move through. Resolved against the scan when it's used, by
    /// that path and then by the folder's name, so a workspace built
    /// around one pack category finds it wherever the pack sits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_folder: Option<String>,
    /// Shuffle from the app-wide favorites list instead of the folder
    /// pick. The folder stays put underneath so switching this off
    /// returns to it. With nothing starred the worker walks the whole
    /// library, which the settings page says out loud.
    pub favorites_only: bool,
    /// How the frame's colours meet the theme. See [`MilkdropColor`].
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

/// What the rotation picker offers: the whole library, the favorites, or
/// one folder. The config keeps the folder and the favorites switch as
/// two fields so a pick survives a round trip through the other; this is
/// the one value the control reads and writes.
#[derive(Clone, Debug, PartialEq)]
enum RotationChoice {
    All,
    Favorites,
    Folder(PathBuf),
}

/// The grade mode a config asks for, in the pass's own terms.
fn grade_mode(color: MilkdropColor) -> GradeMode {
    match color {
        MilkdropColor::Preset => GradeMode::Preset,
        MilkdropColor::Theme => GradeMode::Theme,
        MilkdropColor::Palette => GradeMode::Palette,
        MilkdropColor::Cover => GradeMode::Cover,
    }
}

/// What the engine renders at, from the panel's bounds: device pixels times
/// the config's scale, clamped to what a framebuffer should ever be.
///
/// Pure so the clamp can be tested without a window. Both sides clamp
/// independently, so a very wide, very short panel gets a squarer frame
/// than it asked for and the shader letterboxes it rather than the worker
/// allocating a 4096x1 texture.
fn render_size(width: f32, height: f32, scale_factor: f32, scale: f32) -> (u32, u32) {
    let side = |logical: f32| {
        let device = logical * scale_factor * scale;
        // NaN and negatives both land on the floor: `max` on f32 takes the
        // other operand when one is NaN, and the cast saturates.
        (device.round().max(0.0) as u32).clamp(MIN_SIDE, MAX_SIDE)
    };
    (side(width), side(height))
}

/// Where the album tint stands: the hue the frame is being turned toward
/// and how much of that turn is in force, before the strength setting
/// scales it.
///
/// State rather than a straight read of the seed, because both halves have
/// to move rather than jump. A track change swings the hue across; a stop,
/// or a cover with no colour in it, lets the amount down to nothing
/// instead of dropping the tint in one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct TintAim {
    /// Oklab hue in radians, meaningless while `amount` is zero.
    hue: f32,
    /// How much of the turn is in force, 0 to 1.
    amount: f32,
}

/// A hue turned toward another one by a fraction of the shortest arc
/// between them. Zero holds, one lands on the target.
///
/// The fold is what makes it the shortest arc: hues live on a circle, and
/// the raw difference between one just under a full turn and one just over
/// zero is nearly the whole way round. Rotating that long way walks the
/// colour through every hue in between, which is a rainbow wipe rather
/// than a colour change.
fn turned_hue(from: f32, to: f32, amount: f32) -> f32 {
    let apart = to - from;
    let delta = apart - std::f32::consts::TAU * (apart / std::f32::consts::TAU).round();
    from + delta * amount.clamp(0.0, 1.0)
}

/// One step of the tint's ease toward the cover it should be wearing.
/// `goal` is the cover's hue, None when nothing is playing or the cover
/// had no colour; `step` is the fraction of the ease this frame covers, so
/// the result doesn't depend on the frame rate.
///
/// A tint that isn't showing takes a new hue whole rather than easing to
/// it. There's nothing on screen at zero amount, so easing the hue there
/// spends the ramp-in walking through colours the cover never had.
fn ease_tint(current: TintAim, goal: Option<f32>, step: f32) -> TintAim {
    let step = step.clamp(0.0, 1.0);
    match goal {
        None => TintAim {
            hue: current.hue,
            amount: (current.amount - step).max(0.0),
        },
        Some(hue) if current.amount <= 0.0 => TintAim { hue, amount: step },
        Some(hue) => {
            // A constant angular rate, not a fraction of what's left to
            // travel. Chipping away at the remainder never quite lands,
            // and a hue a thousandth of a radian from its goal is a panel
            // still asking for frames over a colour that stopped moving
            // seconds ago. This one arrives.
            let arc = turned_hue(current.hue, hue, 1.0) - current.hue;
            let travel = (step * std::f32::consts::PI).min(arc.abs());
            TintAim {
                hue: current.hue + arc.signum() * travel,
                amount: (current.amount + step).min(1.0),
            }
        }
    }
}

/// What the layout keeps of a preset: its file name, off whatever was
/// stored. A current layout stores the name already; one from before
/// stored the path, and the last component of that is the same name.
fn preset_name(stored: &str) -> String {
    Path::new(stored)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| stored.to_string())
}

/// The scanned preset a saved name names. Preset names are distinctive
/// enough (every pack writes the author into the file name) that the
/// first match is the preset; two packs holding the same file are the
/// same preset twice.
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

/// The texture and chain a single window holds for this panel, and where
/// the frame stream stands against them.
struct Target {
    /// Which window handed these out. A pop-out moves the panel to another
    /// one, and neither the texture nor the compiled chain travels.
    window: u64,
    texture: UserTextureId,
    width: u32,
    height: u32,
    /// The one-pass chain binding `texture`. Registered on the first paint
    /// after the texture is made, and dropped with it on a resize.
    chain: Option<UserShaderId>,
    /// The newest frame this target has drawn. The engine hands back
    /// anything past it and nothing else, so a UI frame with no new render
    /// behind it costs one atomic load.
    last_seq: u64,
}

/// Everything the paint closure owns. Behind a mutex because the closure
/// runs with a `&mut App` and no handle on the panel, which is the same
/// reason the shader panel keeps its registration out here.
#[derive(Default)]
struct Draw {
    /// The size the last paint worked out for the panel, which is what the
    /// engine gets spawned at. `None` until the first paint with a real
    /// size, which is why the engine isn't started in the constructor.
    size: Option<(u32, u32)>,
    /// A size seen once and not yet acted on. A second frame at the same
    /// size is what commits it, so dragging an edge doesn't reallocate the
    /// framebuffer on every pixel of the drag.
    pending: Option<(u32, u32)>,
    target: Option<Target>,
    /// What the window said about the texture or the chain. A backend with
    /// no shader pipeline says so here, and the body shows it.
    error: Option<String>,
    /// Renders that have gone up as a texture, across every target this
    /// panel has had. Zero after the grace is a worker that says it's
    /// running and has nothing to show for it, which the body reports.
    frames: u64,
}

/// What the control socket reads off the panel. See
/// [`MilkdropPanel::snapshot`].
#[derive(Clone, Debug, serde::Serialize)]
pub struct Snapshot {
    /// The preset on screen, as this machine's path.
    pub preset: Option<PathBuf>,
    pub locked: bool,
    /// How many presets the last scan found.
    pub presets: usize,
    /// `all`, `favorites`, or the rotation folder's path.
    pub rotation: String,
    /// projectM's message for the last preset it refused, until another
    /// one lands.
    pub failed: Option<String>,
    /// The worker's or the window's failure, when there's no frame coming.
    pub error: Option<String>,
    /// `idle` before the first paint spawns the worker, then `starting`,
    /// `running`, or `failed`.
    pub engine: &'static str,
    pub renderer: Option<String>,
    pub projectm_version: Option<String>,
    /// Renders that have gone up as a texture.
    pub frames: u64,
}

pub struct MilkdropPanel {
    state: AppState,
    config: MilkdropConfig,
    /// The worker. `None` until the first paint reports a size; see
    /// [`Draw::size`].
    engine: Option<Engine>,
    /// The presets found under the roots. Scanned on the first render
    /// rather than in the constructor, because restoring a layout builds
    /// every panel in it and a pack is a few thousand `stat` calls.
    library: Option<PresetLibrary>,
    /// The roots as they stood when the scan was taken. See
    /// [`RootsStamp`].
    scanned_stamp: Option<RootsStamp>,
    /// Every folder in the scan that holds presets, and the rotation
    /// picker's row for each. Worked out with the scan rather than per
    /// render: a walk of the whole library per keystroke is the lag.
    folders: Vec<PathBuf>,
    folder_options: Vec<(RotationChoice, SharedString)>,
    /// The edit of the app-wide favorites and folders this panel last
    /// acted on. Another panel, or the settings window, starring a preset
    /// or adding a folder moves the lists; the next render here sees the
    /// generation move and catches the scan and the rotation up.
    lists_gen: u64,
    /// The preset on screen, as this machine's path. Runtime only: the
    /// config keeps its name.
    current: Option<PathBuf>,
    draw: Arc<Mutex<Draw>>,
    /// The preset name on screen and when it went up.
    banner: Option<(String, Instant)>,
    /// A preset projectM refused, shown until the next one lands.
    failed: Option<String>,
    /// Whether the worker is parked. Set at the bottom of a fade-out or
    /// on the first frame of a hold, cleared on play.
    parked: bool,
    /// Whether the pointer is over the panel. The control strip reads it
    /// to fade in; see [`MilkdropPanel::controls`].
    hovered: bool,
    /// When the panel last asked the worker for frames: the spawn, or the
    /// latest resume. The stall notices count from here, so a panel that
    /// sat parked for an hour isn't declared stuck the moment it wakes.
    since: Option<Instant>,
    /// Whether the worker has run for this panel yet. A hold with nothing
    /// ever rendered would pin an empty texture at full opacity, so until
    /// the first run a hold reads as a cut.
    held: bool,
    /// The fade between the visual and the panel background. Driven by the
    /// play state, and the clock the park waits on.
    fade: Fade,
    /// Where the album tint has got to, and when it last stepped. The step
    /// runs off wall time rather than a frame count so a panel at 30 fps
    /// eases at the same speed as one at 60.
    tint: TintAim,
    tint_at: Instant,
    /// The frame-rate field, built on the first visit to Tuning for the
    /// same reason the filter box is.
    fps_scrub: ScrubState,
    /// One scrub per slider on the settings pages.
    duration_scrub: ScrubState,
    sensitivity_scrub: ScrubState,
    scale_scrub: ScrubState,
    fade_scrub: ScrubState,
    tint_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel on every pump tick, which is what carries the
    /// play-state change that parks and unparks the worker.
    _player_changed: Subscription,
    /// Wakes the panel when it takes focus, built on the first render
    /// because it needs a window.
    ///
    /// Focus is the other thing that decides whether the worker sleeps, and
    /// a panel that has faded out and parked renders nothing that would ask
    /// for the next frame. Without this, waking it by clicking on it would
    /// depend on something else happening to repaint the view.
    _focus_wake: Option<Subscription>,
}

impl MilkdropPanel {
    pub fn new(state: AppState, mut config: MilkdropConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        // A layout from before names were the identity holds a path here.
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
            // Settled and fully visible. The panel comes up drawing, and
            // the first render with nothing playing takes it down.
            fade: Fade::default(),
            // Untinted, so the first frame with a cover up eases in
            // rather than opening on a colour.
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
        }
    }

    /// Every root the scan walks: the app's own folder first, then the
    /// app-wide extras.
    fn roots(&self) -> Vec<PathBuf> {
        core_settings::milkdrop_scan_roots()
    }

    /// The preset library, scanned on first ask. A rescan is what the
    /// settings page's Rescan button and a root edit both go through.
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
        // A favorite starred from a panel with other roots is still a
        // favorite here, so it joins the list rather than falling out of
        // the favorites rotation.
        library.extend(&core_settings::milkdrop_favorites());
        self.scanned_stamp = Some(self.roots_stamp());
        // The worker took the library by value when it spawned, so the
        // settings list and what's actually rotating are two different things
        // until this goes out. Presets dropped in while rox is running are
        // the whole reason anyone presses Rescan, so skipping it would leave
        // the count looking right and the panel still showing the idle preset.
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

    /// Work the folder list and its picker rows out of the scan.
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

    /// Each scan root's modification time now.
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

    /// Rescan if a root moved since the scan, or there's been no scan.
    /// This is what the Presets page calls, so a pack dropped in shows
    /// up on the next visit without the page walking the library each
    /// time it draws.
    fn rescan_if_stale(&mut self) {
        if self.scanned_stamp.as_ref() != Some(&self.roots_stamp()) {
            self.rescan();
        }
    }

    /// What the config's rotation means to the engine. Favorites win over
    /// the folder while the switch is on; the folder pick is kept for
    /// when it's off.
    fn rotation(&self) -> Rotation {
        if self.config.favorites_only {
            return Rotation::Set(core_settings::milkdrop_favorites());
        }
        match self.rotation_dir() {
            Some(folder) => Rotation::Folder(folder),
            None => Rotation::All,
        }
    }

    /// The folder the config's rotation names on this machine, found in
    /// the scan by its path under a root and then by its name. None with
    /// no pick, or a pick this scan doesn't hold, which rotates everything
    /// the way a deleted folder always did.
    fn rotation_dir(&self) -> Option<PathBuf> {
        let relative = self.config.rotation_folder.as_deref()?;
        let library = self.library.as_ref()?;
        find_folder_by_relative(&self.folders, library.roots(), relative)
    }

    /// The picker's reading of the config.
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
                // A folder a favorite dragged in from outside every root
                // has no path under one; its name is all there is.
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

    /// The preset on screen, as this machine's path.
    fn current_path(&self) -> Option<PathBuf> {
        self.current.clone()
    }

    fn current_is_favorite(&self) -> bool {
        self.current_path()
            .is_some_and(|path| core_settings::is_milkdrop_favorite(&path))
    }

    /// Star or unstar a preset. The write goes to the app-wide list; the
    /// render after this one sees the generation move and does the rest,
    /// which is the same path a change from another panel takes.
    fn toggle_favorite(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let on = !core_settings::is_milkdrop_favorite(&path);
        core_settings::set_milkdrop_favorite(&path, on);
        cx.notify();
    }

    /// Show the preset's file in the system file manager: the way to find
    /// out what a name in the banner actually is, and to get at the file
    /// for editing or sharing.
    fn reveal_preset(&self, cx: &mut App) {
        if let Some(path) = self.current_path() {
            cx.reveal_path(&path);
        }
    }

    /// Catch up with a favorites list that moved since this panel last
    /// looked. The favorites are folded into the library already held,
    /// which stats the favorites and nothing else: a full rescan here was
    /// a few thousand stats per star, which is a click that hangs. The
    /// rotation is re-sent whether or not the library grew, since a
    /// favorites rotation is the list itself.
    fn follow_lists(&mut self) {
        let gen = core_settings::milkdrop_gen();
        if gen == self.lists_gen {
            return;
        }
        self.lists_gen = gen;
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

    /// Open the folder rox scans in the system file manager. Nothing
    /// creates it, so a fresh install gets it made here rather than
    /// handing the platform a path it will refuse.
    fn open_presets_folder(cx: &mut App) {
        let folder = rox_core::settings::milkdrop_dir().join("presets");
        std::fs::create_dir_all(&folder).ok();
        cx.open_with_system(&folder);
    }

    /// Send a command, if there's a worker to take it. Everything the
    /// settings pages and the menu do goes through here, so a panel whose
    /// engine hasn't started yet drops the command rather than queueing a
    /// pile of them against a size nobody has measured.
    fn send(&self, command: Command) {
        if let Some(engine) = self.engine.as_ref() {
            engine.send(command);
        }
    }

    /// Start the worker once a paint has said how big the panel is.
    ///
    /// The library goes in by value, so the worker's copy is a snapshot: a
    /// rescan after this point changes what the settings page lists and not
    /// what the worker shuffles through, until the panel is rebuilt. That's
    /// the same deal every other cached scan in the app makes, and the
    /// alternative is a lock on the hot path of a render loop.
    fn start(&mut self, size: (u32, u32), cx: &mut Context<Self>) {
        let library = self.library().clone();
        // The saved preset goes in with the spawn so the worker comes up
        // on it rather than shuffling one and switching a frame later.
        let preset = self.restore_path();
        let engine = Engine::spawn(EngineOptions {
            feed: self.state.player.read(cx).feed(),
            library,
            preset,
            fps: self.config.fps,
            width: size.0,
            height: size.1,
        });
        // The knobs the config carries, pushed once at startup so a
        // restored panel comes up the way it was left rather than at
        // projectM's defaults until somebody touches a slider.
        engine.send(Command::SetPresetDuration(self.config.duration_secs));
        engine.send(Command::SetBeatSensitivity(self.config.beat_sensitivity));
        engine.send(Command::SetHardCut(self.config.hard_cuts));
        engine.send(Command::SetLocked(self.config.locked));
        // The rotation goes in with the rest. Without it a restored config
        // draws its folder pick on the page while the worker quietly walks
        // the whole library.
        engine.send(Command::SetRotation(self.rotation()));
        self.engine = Some(engine);
        self.since = Some(Instant::now());
    }

    /// The preset a restored layout should come up on: the saved name,
    /// found in the scan. What lets a workspace saved on one machine land
    /// on the same preset on another, wherever the pack lives there.
    fn restore_path(&mut self) -> Option<PathBuf> {
        let name = self.config.preset.clone()?;
        find_by_name(self.library().presets(), &name)
    }

    /// Note the preset that's up: the path for this panel, the name for
    /// the layout.
    fn note_preset(&mut self, path: &Path) {
        self.config.preset = Some(preset_name(&path.to_string_lossy()));
        self.current = Some(path.to_path_buf());
    }

    /// How long a fade runs, from the config. Zero with the switch off,
    /// which is the same cut a saved zero duration already gave.
    fn fade_duration(&self) -> Duration {
        if !self.config.fade {
            return Duration::ZERO;
        }
        Duration::from_secs_f32(self.config.fade_secs.clamp(0.0, FADE_MAX))
    }

    /// Where the fade stands right now: 1.0 fully drawn, 0.0 fully gone.
    fn opacity(&self) -> f32 {
        self.fade.opacity(self.fade_duration())
    }

    /// Whether a fade is still moving. What keeps the panel asking for
    /// frames after the event that started the fade is long gone.
    fn fading(&self) -> bool {
        self.fade.running(self.fade_duration())
    }

    /// Head for an opacity, starting from wherever the current fade got
    /// to. A stop half a second into a fade-in turns around there rather
    /// than snapping back to full and dropping.
    fn fade_to(&mut self, to: f32) {
        let duration = self.fade_duration();
        self.fade.retarget(to, duration);
    }

    /// Follow the audio. Playing runs the worker; a pause or a stop either
    /// holds the last frame or fades the visual out, whichever the config
    /// says, and parks the worker under it. The two mean different things
    /// to look at: a held frame reads as the track waiting, a fade reads
    /// as the panel letting go, so it's the user's pick rather than a rule
    /// about pause versus stop.
    ///
    /// A held frame costs nothing to keep. The texture holding the last
    /// render is the panel's own until it's released or the window closes,
    /// so every frame after the park samples what's already up there, with
    /// no render, no readback and no upload. On the fade path the fade is
    /// the park timer rather than a second clock beside it. A parked
    /// worker publishes no frames, so pausing on the event would freeze
    /// the picture the fade is meant to be taking away; the pause goes out
    /// at the bottom of the fade, when there's nothing left on screen to
    /// hold still.
    ///
    /// With `run_focused` on, a held panel's own focus keeps it running,
    /// since somebody sitting on the panel picking presets in silence is
    /// using it and wants to see it move. It's a switch because focus is
    /// sticky: the dock focuses the active tab and a click anywhere on the
    /// panel takes focus, so a panel stays focused long after anyone last
    /// used it, and a paused track with the visual still cycling under it
    /// reads as the pause not having taken.
    fn park_or_resume(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            return;
        }
        let playing = self.state.player.read(cx).is_playing();
        let hold = !self.config.fade;
        // Focus only stands in for the audio where the frame would
        // otherwise sit still under somebody using the panel; a fading
        // panel follows the audio alone.
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
        // Never having run is the one hold that isn't: there's no frame
        // to keep, and an empty texture at full opacity is a black panel.
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

    /// Step the album tint toward the cover that's up, and say whether it
    /// still has somewhere to go.
    ///
    /// The seed comes from the palette's own per-player store, which the
    /// backdrop bake writes on every track change. Reading it there rather
    /// than the theme's accent means the frame follows the cover whether
    /// or not the user runs the tinted theme mode, and a cover with no
    /// colour in it reads the same as nothing playing: the tint eases off
    /// instead of snapping.
    fn step_tint(&mut self) -> bool {
        let goal = palette::seed(self.state.player.entity_id())
            .and_then(|seed| seed.primary)
            .map(|primary| palette::rgba_to_oklch(primary).2)
            // A tint nobody asked for shouldn't sit there easing; strength
            // zero lets go of the hue the same way a stop does.
            .filter(|_| self.config.tint_strength > 0.0);
        let elapsed = self.tint_at.elapsed();
        self.tint_at = Instant::now();
        let step = elapsed.as_secs_f32() / TINT_EASE.as_secs_f32();
        self.tint = ease_tint(self.tint, goal, step);
        match goal {
            // The arc still to travel, folded the same way the step folds
            // it: a hue that has wrapped past a full turn is no further
            // from its goal than one that hasn't.
            Some(hue) => {
                self.tint.amount < 1.0
                    || (turned_hue(self.tint.hue, hue, 1.0) - self.tint.hue).abs() > 1e-3
            }
            None => self.tint.amount > 0.0,
        }
    }

    /// Take what the worker has to say since the last frame: which preset
    /// came up, and which one wouldn't load.
    fn drain_events(&mut self) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        for event in engine.take_events() {
            match event {
                Event::PresetChanged(path) => {
                    self.note_preset(&path);
                    self.failed = None;
                    // A preset that runs here has earned another go at a
                    // thumbnail, whatever the thumbnailer thought of it.
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

    /// What the panel says over the frame when there's no frame coming: the
    /// worker's failure message, or the window's. `None` while it renders.
    fn error(&self) -> Option<String> {
        if let Some(error) = self.draw.lock().unwrap().error.clone() {
            return Some(error);
        }
        match self.engine.as_ref().map(Engine::status) {
            Some(Status::Failed(message)) => Some(message),
            _ => None,
        }
    }

    /// What the panel says when the worker is alive and nothing is coming:
    /// a start still waiting on the driver past the grace, or an engine
    /// that reports running and has never delivered a frame. Both used to
    /// be a black panel with no word, which on the machine it happens on
    /// looks exactly like the feature working with the lights off. The
    /// headline and the detail under it, or `None` while there's nothing
    /// to say.
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

    /// Put a preset up by path, scanned or not. The picker's pick and the
    /// control socket's load both land here.
    pub fn load_preset(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.note_preset(&path);
        self.send(Command::LoadPreset { path, smooth: true });
        cx.notify();
    }

    /// Hold the preset that's up: no timed switch, no cut on a beat.
    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        self.config.locked = locked;
        self.send(Command::SetLocked(locked));
        cx.notify();
    }

    /// Scan the roots again and hand the worker what changed. The
    /// settings page's Rescan button and the control socket come through
    /// here.
    pub fn rescan_presets(&mut self, cx: &mut Context<Self>) {
        self.rescan();
        cx.notify();
    }

    /// Step the rotation forward, a hard cut. The context menu's Random
    /// and the control socket's next.
    pub fn next_preset(&self) {
        self.send(Command::NextPreset { smooth: false });
    }

    /// Back along the trail, a hard cut.
    pub fn previous_preset(&self) {
        self.send(Command::PreviousPreset { smooth: false });
    }

    /// The newest frame the worker rendered, as projectM drew it: before
    /// the tint, the grade, and the flips the paint puts on top. `None`
    /// until the worker has delivered once.
    pub fn latest_frame(&self) -> Option<rox_milkdrop::Frame> {
        self.engine.as_ref()?.frame_after(0)
    }

    /// The panel's state for the control socket's debug scope (ADR 22):
    /// what's up, whether it's held, and what projectM last refused, so a
    /// script can tell a preset landed without a screenshot. It reflects
    /// the events the last paint drained, so a load reads back once the
    /// panel has drawn a frame after it.
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

    /// Browse for another preset root. Directories only: a pack is a folder
    /// of a few thousand files, and pointing at one of them would be
    /// pointing at a preset, which the list already does.
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
            // The list is app-wide; the write moves the generation, and
            // this panel's next render rescans like every other one.
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

/// The thumbnail service every preset browser in the app draws from: the
/// engine-driving [`rox_milkdrop::thumbs::Thumbnailer`] behind the seam's
/// trait, caching under the app's Milkdrop folder. One for the app, since
/// one tiny engine is plenty and two would race for the same files.
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

    fn gen(&self) -> u64 {
        self.0.gen()
    }

    fn loaded(&self, preset: &Path) {
        self.0.loaded(preset);
    }
}

/// A Milkdrop panel as the preset picker window sees it. Weak, so a
/// picker left open over a panel that closed does nothing rather than
/// keeping the panel alive.
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

/// What the pass needs to know beyond the frame itself: how far the fade
/// has got, and where the album tint is pointing. Grouped because they
/// travel together into the paint closure and out again as signal slots.
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

/// The signal slots the panel's own flips ride in, past the grade's.
const SLOT_FLIP_H: usize = 14;
const SLOT_FLIP_V: usize = 15;

/// One frame of the visual: keep the texture the right size, push the
/// newest render into it, and record the pass that draws it.
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

    // The first real size is what starts the worker, and the panel is the
    // only one that can do that: this closure has no handle on it.
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
        // A drag that came back where it started leaves a size sitting in
        // `pending` that nothing will ever commit, and the next real resize
        // to that same size would skip its own debounce.
        draw.pending = None;
    } else {
        // A target in the wrong window isn't a resize, it's a pop-out, and
        // there's nothing on screen until a new one exists. Only a genuine
        // size change waits out the debounce.
        let moved = draw
            .target
            .as_ref()
            .is_none_or(|target| target.window != window_id);
        if moved || draw.pending == Some(want) {
            draw.pending = None;
            if let Some(old) = draw.target.take() {
                // Only this window's own registry can free it. A texture
                // left behind in the window a panel was popped out of dies
                // when that window closes, which is the same life a
                // registered image has there.
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
            // First sighting of this size. Ask for another frame so the
            // second sighting arrives even if nothing else is moving, and
            // keep drawing the old target meanwhile: a resize that blanks
            // the panel for a frame reads as a flicker.
            draw.pending = Some(want);
            window.request_animation_frame();
        }
    }

    // The newest render, if there is one past what's already up there. A
    // frame from before a resize is dropped rather than stretched: its seq
    // still moves on, so it's dropped once and not re-fetched every frame.
    let uploaded = {
        let Some(target) = draw.target.as_mut() else {
            return;
        };
        match engine.frame_after(target.last_seq) {
            Some(frame) => {
                target.last_seq = frame.seq;
                let fits = frame.width == target.width && frame.height == target.height;
                if fits {
                    if let Err(message) = window.update_user_texture(target.texture, frame.rgba8) {
                        draw.error = Some(message);
                        cx.notify(panel);
                        return;
                    }
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

    // A chain that binds an asset can only run as a screen pass, so this is
    // always the region branch. The entity id keys the region's scratch
    // texture, the way the shader panel keys its feedback buffer.
    let meta = surface::meta_slots(window, cx);
    // Slot 0 is the fade. Element opacity doesn't reach a shader region, so
    // the only way down to the background is through the pass itself.
    let mut signals = [0.0f32; 16];
    signals[grade::SLOT_FADE] = look.fade;
    signals[grade::SLOT_HUE] = look.hue;
    signals[grade::SLOT_TINT] = look.tint;
    look.grade.write(&mut signals);
    signals[SLOT_FLIP_H] = if look.flip_horizontal { 1.0 } else { 0.0 };
    signals[SLOT_FLIP_V] = if look.flip_vertical { 1.0 } else { 0.0 };
    window.paint_screen_shader(bounds, shader, panel.as_u64(), signals, meta);

    if animate {
        // A docked panel renders cached, so a recorded pass replays with the
        // values it was recorded with. Sixty frames a second needs the view
        // dirtied sixty times a second, and this is the cheap wake: it
        // notifies this view rather than rebuilding the window.
        window.request_animation_frame();
    }
}

impl PanelSettings for MilkdropPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    /// On, unlike the shader panel's. A surface shader over a Milkdrop
    /// frame is half the reason the frame goes through a chain at all:
    /// the visual composes like any other panel body.
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
    /// The Presets page: where presets are found, how many turned up, and
    /// the list itself.
    fn presets_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Opening the page is what picks up a pack dropped in since the
        // panel last looked. See [`RootsStamp`] for why it's a stamp
        // check and not a scan per frame.
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

        // The rotation options: everything, then every folder that holds
        // presets of its own, named relative to the root it was found in.
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
        // Favorites picked with nothing starred is the one pick that does
        // something other than what it says, so the page says what.
        let no_favorites = (self.config.favorites_only && favorites.is_empty())
            .then(|| rox_i18n::t!("milkdrop-no-favorites"));

        // The preset on screen, with the two things worth doing to it
        // from here: star it, and find its file.
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
            // The list itself is the picker window's: a settings page
            // is no place for a hundred thousand rows.
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

        // 1 to 120 seconds, the span a switch is worth having. Linear: the
        // interesting half of it is the short end and the long end is one
        // decision ("leave it alone"), so a log strip would spend most of
        // its travel on the part nobody scrubs through.
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
            // A block: the name runs long and the buttons need the
            // width, so neither fits a row's control slot.
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

    /// What the Presets section says before there are any presets: that the
    /// scan found nothing, where to get some, and a way into the folder
    /// they go in. The Rescan button sits in the section header above this.
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

        // The same strip the Appearance page's backdrop rate uses, so the
        // two visuals tune alike.
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

        // The scale slider drives the same resize path the segmented
        // control did, and the paint closure is what actually commits it:
        // a size has to be measured twice running before the framebuffer is
        // reallocated, so a drag across the strip changes a float per frame
        // and reallocates once, when the drag stops moving.
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
                // Snapped to whole percent so the readout holds still
                // under a drag and the typed value comes back unchanged.
                let percent = (SCALE_MIN + fraction * (SCALE_MAX - SCALE_MIN)) * 100.0;
                this.config.scale = percent.round() / 100.0;
                cx.notify();
            },
            cx,
        );

        // Percent, so the readout is a plain number and the strength the
        // shader gets is the fraction behind it.
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

        // Seconds, snapped to a tenth so the readout holds still under a
        // drag and a typed value comes back unchanged.
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
                // The tint turns the preset's own hues, and Palette and
                // Cover replace them, so the slider is only asked where
                // it would do something.
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
                // The duration is only a question once there's a fade to
                // time, and focus only stands in for the audio on a held
                // frame, so each is asked only where it does something.
                // Dimming either in place would leave a control that
                // still moves.
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
                                // Switched on between changes, the name
                                // has to come up now, not on the next one.
                                if on && this.banner.is_none() {
                                    if let Some(path) = this.current_path() {
                                        this.banner = Some((preset_label(&path), Instant::now()));
                                    }
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

    /// The layout dump stores the panel's config, the preset it was on
    /// included; the builder registered in `workspace::register_panels`
    /// reads it back.
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
                        // Nothing creates the folder, so the first open on a
                        // fresh install makes it rather than handing the
                        // platform a path it will refuse.
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
        // The size the last paint measured. `Some` means a paint has run
        // with a real size, which is the cue to start the worker.
        let size = self.draw.lock().unwrap().size;
        if let (Some(size), None) = (size, self.engine.as_ref()) {
            self.start(size, cx);
        }
        self.drain_events();
        self.follow_lists();
        self.park_or_resume(window, cx);
        let tinting = self.step_tint();

        // Whether the panel asks for the next frame. Starting counts: the
        // first render lands a few frames after the worker goes up, and
        // without the ask the panel would sit on an empty texture until
        // something else woke it. A fade counts too: it outlives the stop
        // that started it, and it's the fade's own frames that carry the
        // opacity down to zero and park the worker there.
        let fading = self.fading();
        let animate = fading
            || tinting
            || (!self.parked
                && matches!(
                    self.engine.as_ref().map(Engine::status),
                    Some(Status::Starting) | Some(Status::Running { .. })
                ));

        // The fade happens inside the pass, not under a scrim: the shader
        // blends toward whatever the body already painted, so the bottom of
        // the fade is the empty panel exactly and dropping the canvas there
        // changes nothing on screen. See [`FRAME_WGSL`] and [`fade::mix`].
        let opacity = self.opacity();
        let show = opacity > 0.0 || fading;
        // Read inside the themed body, so a panel wearing its own theme
        // grades against that theme's background and not the window's.
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
        // A failure or a stall, over the body. The controls stay up under a
        // stall: pressing Next and seeing a name land is how somebody tells
        // a worker that's alive from one that isn't.
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
                    // The controls fade in on hover anywhere over the panel, not
                    // just over themselves, or nobody would find them. Tracked here
                    // rather than through a group: the strip is deferred, and a
                    // group's hitbox is only registered while its own subtree
                    // paints, so a deferred child never sees it.
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
                    // Everything over the visual is deferred. The frame is a shader
                    // region, and the DirectX renderer runs regions once, at the
                    // deferred-draw boundary, after every ordinary primitive: a
                    // banner or an error painted in tree order sat under the
                    // opaque frame there, which is how a Windows user with no
                    // usable OpenGL got a black panel and no word about it. Blade
                    // runs regions in paint order and doesn't care either way.
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
    /// The transport strip over the visual: back, forward, random, and
    /// the star. Invisible until the pointer is over the panel, so the
    /// visual stays the visual; the buttons are still there under the
    /// zero opacity, and moving onto the panel is what brings them up.
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
        // The preset lock and the dock pin are separate switches that both
        // used to want the key `locked`; the rename is what keeps them so.
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

    /// A layout saved before favorites and the grade existed comes back
    /// walking its folder in the preset's own colours on the dark theme,
    /// which is what it did before: the theme grade is the identity there.
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
    /// The turn takes the short way round the hue circle, both ends are
    /// fixed points, and the fraction lands in between.
    #[test]
    fn a_hue_turns_the_short_way_round() {
        use std::f32::consts::{PI, TAU};

        // Nothing at zero, everything at one.
        assert_eq!(turned_hue(1.0, 2.5, 0.0), 1.0);
        assert!((turned_hue(1.0, 2.5, 1.0) - 2.5).abs() < 1e-5);
        // Half way is half the arc.
        assert!((turned_hue(1.0, 2.0, 0.5) - 1.5).abs() < 1e-5);

        // Across the wrap: a hue just under a full turn and one just over
        // zero are neighbours, so the turn goes forward by a hair and not
        // backwards through every hue there is.
        let turned = turned_hue(TAU - 0.1, 0.1, 1.0);
        assert!(turned > TAU - 0.1, "went the long way: {turned}");
        assert!((turned - (TAU + 0.1)).abs() < 1e-5);

        // The other direction across the same seam.
        let back = turned_hue(0.1, TAU - 0.1, 1.0);
        assert!(back < 0.1, "went the long way: {back}");

        // A goal an exact half turn away is the one case with no short
        // way; either arc is the same length, so all that matters is that
        // it moves by half a turn and stays finite.
        let opposite = turned_hue(0.0, PI, 1.0);
        assert!((opposite.abs() - PI).abs() < 1e-5, "moved {opposite}");

        // Out-of-range fractions clamp instead of overshooting the goal.
        assert!((turned_hue(1.0, 2.0, 5.0) - 2.0).abs() < 1e-5);
        assert!((turned_hue(1.0, 2.0, -5.0) - 1.0).abs() < 1e-5);
    }

    /// The ease ramps in on a cover, slides across on a track change, and
    /// lets go when there's nothing playing.
    #[test]
    fn the_tint_eases_in_across_and_out() {
        use std::f32::consts::PI;

        // From nothing, the first step takes the cover's hue whole and
        // ramps the amount up from zero. Easing the hue here would spend
        // the ramp-in walking through colours the cover never had.
        let first = ease_tint(TintAim::default(), Some(2.0), 0.25);
        assert_eq!(first.hue, 2.0);
        assert_eq!(first.amount, 0.25);

        // Showing already, so the next cover slides across.
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

        // It arrives rather than creeping: a step big enough to cover
        // what's left lands on the goal exactly and doesn't overshoot it.
        let landed = ease_tint(showing, Some(1.0), 1.0);
        assert_eq!(landed.hue, 1.0);

        // Nothing playing, or a cover with no colour in it: let go rather
        // than drop it, and hold the hue while it goes so the last of the
        // tint stays the colour it was.
        let letting_go = ease_tint(showing, None, 0.25);
        assert_eq!(letting_go.hue, 0.0);
        assert_eq!(letting_go.amount, 0.75);

        // Both ends stop where they should, however big the step.
        assert_eq!(ease_tint(showing, None, 10.0).amount, 0.0);
        assert_eq!(ease_tint(showing, Some(0.0), 10.0).amount, 1.0);
        assert_eq!(ease_tint(TintAim::default(), None, 0.5).amount, 0.0);
    }

    /// A layout saved before the fade existed comes back at the calm
    /// default rather than at no fade, which is what a missing float
    /// would read as if the container default weren't doing the work.
    #[test]
    fn a_dump_without_a_fade_reads_as_the_default() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fade_secs").expect("the fade was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.fade_secs, 1.0);
        assert_eq!(read.scale, 0.75);
    }

    /// The switch and the duration are two fields, so a layout saved
    /// before the switch existed comes back on the default, holding, and
    /// still has its duration waiting for the day the fade is switched
    /// on.
    #[test]
    fn a_dump_without_the_fade_switch_keeps_its_duration() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fade").expect("the switch was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert!(!read.fade);
        assert_eq!(read.fade_secs, 2.5);
    }

    /// A dump written before the tint existed comes back tinting at the
    /// default rather than at zero, which is what a missing float would
    /// read as without the container default.
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

    /// A dump written before a field existed reads back at the default
    /// rather than failing the whole layout.
    #[test]
    fn a_dump_without_a_field_reads_as_the_default() {
        let mut dumped = serde_json::to_value(tuned()).expect("dump");
        let object = dumped.as_object_mut().expect("object");
        object.remove("fps").expect("fps was written");

        let read: MilkdropConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.fps, 60);
        // And what was in the dump still came through.
        assert_eq!(read.duration_secs, 12.0);
    }

    /// The preset stays out of the dump when there isn't one, so a panel
    /// nobody has touched doesn't write a null into every layout. The
    /// rotation folder rides the same rule: no pick, no key.
    #[test]
    fn no_preset_writes_no_key() {
        let dumped = serde_json::to_value(MilkdropConfig::default()).expect("dump");
        assert!(dumped.get("preset").is_none());
        assert!(dumped.get("rotation_folder").is_none());
    }

    /// The rotation picker's labels: relative to the root the folder was
    /// found under, because the absolute path is mostly the part every
    /// option on the list has in common.
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
        // A folder under no root at all still has to say something, and
        // the whole path is the only honest answer left.
        assert_eq!(
            folder_label(Path::new("/elsewhere/Fractal"), &roots),
            "/elsewhere/Fractal"
        );
        // A root itself strips to nothing, which would be a blank row.
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

    /// The pass is a string constant, so nothing but a running window
    /// would ever compile it. This does, against the same template the
    /// window composes it into.
    #[test]
    fn the_frame_pass_compiles_with_the_grade_in_scope() {
        crate::panel::shader::validate_frame_pass(&grade::wgsl(FRAME_WGSL), &["frame"])
            .expect("the Milkdrop panel's pass validates");
    }

    #[test]
    fn the_render_size_clamps_both_sides() {
        // The settled case: device pixels, straight through.
        assert_eq!(render_size(800.0, 450.0, 2.0, 1.0), (1600, 900));
        // Scale multiplies before the clamp.
        assert_eq!(render_size(800.0, 400.0, 1.0, 0.5), (400, 200));
        // A panel dragged to a sliver still gets a framebuffer worth
        // allocating.
        assert_eq!(render_size(4000.0, 3.0, 1.0, 1.0), (4000, MIN_SIDE));
        // And a wall-sized one stops where the readback bill does.
        assert_eq!(render_size(6000.0, 6000.0, 2.0, 1.0), (MAX_SIDE, MAX_SIDE));
        // Zero and negative both land on the floor rather than on a
        // framebuffer nothing can be allocated for.
        assert_eq!(render_size(0.0, -10.0, 1.0, 1.0), (MIN_SIDE, MIN_SIDE));
    }

    /// The relative form is what travels: under its root, forward
    /// slashes, and the shortest when roots overlap.
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

    /// The layout keeps a name and nothing else, and a layout from before
    /// this, which kept the path, reads as the same name. The folder is a
    /// path under the root, never from it.
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

    /// On the other machine the pack is wherever it is: the name finds
    /// the preset, and a name the scan doesn't hold finds nothing rather
    /// than something.
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

    /// The rotation folder relocates the same way the preset does.
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
