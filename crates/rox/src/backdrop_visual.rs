//! The Milkdrop visual behind the whole app: one engine, every window.
//!
//! A live MilkDrop frame composited over ADR 10's blurred cover backdrop at a
//! user-set strength while something plays. Zero strength is the cover alone.
//! The engine sits under ADR 28.
//!
//! ## One engine
//!
//! [`rox_milkdrop::Engine`] renders into a private GL context and reads the
//! pixels back, about 2.5 ms a frame at 1080p, so there's one engine at one
//! size and every window uploads the same frame into its own texture.
//! Textures and chains belong to their window and can't be shared.
//!
//! The size is the per-axis maximum over the painting windows, times the
//! render scale. Each window draws it cover-fit, and the max guarantees
//! cover-fit never upscales; a small child window gets a crop of the middle.
//!
//! ## Composite
//!
//! The pass writes `vec4(rgb * a, a)`, `a` being strength times fade, and the
//! region pipeline blends premultiplied-over, so the strength is exactly the
//! mix between the cover and the frame.
//!
//! ## Cost
//!
//! This runs whenever audio plays, so the defaults are half scale at 30 fps,
//! about an eighth of a full-size 60 fps panel. A parked worker costs one
//! atomic load per window per frame.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Entity, SharedString, UserShaderChain, UserShaderId, UserShaderPass,
    UserTextureId, Window, canvas, div, prelude::*, px,
};

use rox_core::settings::{self, BackdropVisualConfig, MilkdropColor, Settings};
use rox_design::palette::{self, Mode};
use rox_milkdrop::library::Shuffle;
use rox_milkdrop::{Command, Engine, EngineOptions, Event, PresetLibrary, Rotation, Status};
use rox_panel_api::panel::shader as surface;
use rox_panel_api::preset_browser::{
    PresetHost, find_folder_by_relative, folder_label, relative_to_roots,
};
use rox_panel_kit::fade::{self, Fade};
use rox_panel_kit::grade::{self, Grade, GradeMode};
use rox_services::player::Player;

/// The floor spares projectM a 4x1 framebuffer; the ceiling keeps a 5K window
/// from a 30 MB readback per frame.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 4096;

/// The fade under hold, where only the switch and first play transition.
/// Fixed: a backdrop that takes its time to appear reads as a bug.
const FADE: Duration = Duration::from_millis(700);

/// A window that stops painting must stop inflating the shared render size.
const WINDOW_STALE: Duration = Duration::from_secs(1);

/// Keys the region's scratch texture. There's no view to take an entity id
/// from, so it's a value the entity counter never reaches.
const INSTANCE: u64 = u64::MAX;

/// Sample the frame, cover-fit it, grade it, and carry the strength as
/// premultiplied alpha. Cover rather than the panel's aspect-fit, because a
/// backdrop has to reach every corner.
///
/// The grade makes light themes work: a black-backed frame over a pale wash
/// is a grey smear until its lightness flips. The maths is
/// [`rox_panel_kit::grade`]'s, prepended to this source.
const FRAME_WGSL: &str = "
fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let weight = clamp(params.signals[0].x, 0.0, 1.0);
    if (weight <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let frame_size = max(vec2<f32>(textureDimensions(frame)), vec2<f32>(1.0, 1.0));
    let bounds = max(params.resolution, vec2<f32>(1.0, 1.0));
    let fill = max(bounds.x / frame_size.x, bounds.y / frame_size.y);
    let covered = frame_size * fill;
    let margin = (covered - bounds) * 0.5;
    var source = clamp((uv * bounds + margin) / covered, vec2<f32>(0.0), vec2<f32>(1.0));
    // Slots 14 and 15 mirror the sample, which mirrors the frame.
    if (params.signals[3].z > 0.5) {
        source.x = 1.0 - source.x;
    }
    if (params.signals[3].w > 0.5) {
        source.y = 1.0 - source.y;
    }
    let graded = grade(textureSample(frame, samp, source).rgb);
    return vec4<f32>(graded * weight, weight);
}
";

/// Past the grade's slots, the same two the Milkdrop panel reads.
const SLOT_FLIP_H: usize = 14;
const SLOT_FLIP_V: usize = 15;

struct Target {
    texture: UserTextureId,
    width: u32,
    height: u32,
    /// Registered on the first paint after the texture, dropped with it on resize.
    chain: Option<UserShaderId>,
    /// So a UI frame with no new render behind it costs one atomic load.
    last_seq: u64,
}

/// One window's slice: its wanted size, when it last said so, and its own
/// fade. Per window because each follows its own workspace's player and gate,
/// so a settings window fading out can't take the workspace's visual.
struct Pane {
    size: (u32, u32),
    at: Instant,
    fade: Fade,
    /// Kept so the lit test reads the same clock the paint retargeted with.
    duration: Duration,
    /// What the park keys on under hold, where lit isn't a reason to render.
    playing: bool,
    /// Under hold a pane that never saw playback stays dark rather than pinning
    /// an empty texture over the cover.
    held: bool,
}

impl Pane {
    /// Fading included. A dark pane doesn't count toward the shared size.
    fn lit(&self, now: Instant) -> bool {
        (self.fade.to > 0.0 || self.fade.running(self.duration)) && self.fresh(now)
    }

    fn fresh(&self, now: Instant) -> bool {
        now.duration_since(self.at) < WINDOW_STALE
    }
}

/// Not an entity: the paint runs from the backdrop's shade hook, which gets
/// only a window and an app.
#[derive(Default)]
struct Visual {
    engine: Option<Engine>,
    /// The feed is fixed at spawn, so switching players needs a new context, only from parked.
    driving: Option<gpui::EntityId>,
    panes: HashMap<u64, Pane>,
    targets: HashMap<u64, Target>,
    /// A size is committed on its second sighting, so a window drag doesn't
    /// reallocate the framebuffer every pixel.
    size: Option<(u32, u32)>,
    pending: Option<(u32, u32)>,
    parked: bool,
    error: Option<String>,
    /// Rescanned when the scan folders change. There's no Rescan button, so new
    /// packs in a folder already walked appear on the next launch.
    library: Option<PresetLibrary>,
    playing: HashMap<gpui::EntityId, bool>,
    current: Option<PathBuf>,
    /// One compare per paint instead of re-sending every command.
    applied: Option<Applied>,
    folders: Option<Vec<PathBuf>>,
    folder_options: Option<Arc<Vec<(String, SharedString)>>>,
}

#[derive(Clone, Debug, PartialEq)]
struct Applied {
    favorites_only: bool,
    lists_gen: u64,
    locked: bool,
    duration_secs: f64,
    fps: u32,
    beat_sensitivity: f32,
    hard_cuts: bool,
    rotation_folder: Option<String>,
}

impl Applied {
    fn of(config: &BackdropVisualConfig) -> Applied {
        Applied {
            favorites_only: config.favorites_only,
            lists_gen: settings::milkdrop_gen(),
            locked: config.locked,
            duration_secs: config.duration_secs,
            fps: config.fps,
            beat_sensitivity: config.beat_sensitivity,
            hard_cuts: config.hard_cuts,
            rotation_folder: config.rotation_folder.clone(),
        }
    }
}

/// Favorites, the picked folder if the scan holds it, else everything.
fn rotation(visual: &mut Visual, config: &BackdropVisualConfig) -> Rotation {
    if config.favorites_only {
        return Rotation::Set(settings::milkdrop_favorites());
    }
    match rotation_dir(visual, config) {
        Some(folder) => Rotation::Folder(folder),
        None => Rotation::All,
    }
}

/// Matched by path under a root, then by name. A missing pick rotates everything.
fn rotation_dir(visual: &mut Visual, config: &BackdropVisualConfig) -> Option<PathBuf> {
    let relative = config.rotation_folder.as_deref()?;
    let roots = library(visual).roots().to_vec();
    find_folder_by_relative(folders(visual), &roots, relative)
}

fn folders(visual: &mut Visual) -> &Vec<PathBuf> {
    if visual.folders.is_none() {
        visual.folders = Some(library(visual).folders());
    }
    visual.folders.as_ref().expect("just listed")
}

fn set_library(visual: &mut Visual, library: PresetLibrary) {
    visual.library = Some(library);
    visual.folders = None;
    visual.folder_options = None;
}

/// The rotation picker's folder rows, keyed by path under their root. Built once per scan.
pub(crate) fn rotation_folders() -> Arc<Vec<(String, SharedString)>> {
    let mut guard = visual();
    let visual = guard.as_mut().expect("initialised on first lock");
    if let Some(options) = visual.folder_options.clone() {
        return options;
    }
    let roots = library(visual).roots().to_vec();
    let options: Vec<(String, SharedString)> = folders(visual)
        .iter()
        .filter_map(|folder| {
            let key = relative_to_roots(folder, &roots)?;
            Some((key, SharedString::from(folder_label(folder, &roots))))
        })
        .collect();
    let options = Arc::new(options);
    visual.folder_options = Some(options.clone());
    options
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BackdropRotation {
    All,
    Favorites,
    Folder(String),
}

impl BackdropRotation {
    pub(crate) fn of(config: &BackdropVisualConfig) -> BackdropRotation {
        if config.favorites_only {
            return BackdropRotation::Favorites;
        }
        match config.rotation_folder.clone() {
            Some(folder) => BackdropRotation::Folder(folder),
            None => BackdropRotation::All,
        }
    }

    /// All clears the folder too: it's a choice to rotate everything.
    pub(crate) fn apply(self, config: &mut BackdropVisualConfig) {
        match self {
            BackdropRotation::All => {
                config.favorites_only = false;
                config.rotation_folder = None;
            }
            BackdropRotation::Favorites => config.favorites_only = true,
            BackdropRotation::Folder(folder) => {
                config.favorites_only = false;
                config.rotation_folder = Some(folder);
            }
        }
    }
}

fn grade_mode(color: MilkdropColor) -> GradeMode {
    match color {
        MilkdropColor::Preset => GradeMode::Preset,
        MilkdropColor::Theme => GradeMode::Theme,
        MilkdropColor::Palette => GradeMode::Palette,
        MilkdropColor::Cover => GradeMode::Cover,
    }
}

/// Every panel's folders plus whatever is starred.
fn scan_library() -> PresetLibrary {
    let roots = settings::milkdrop_scan_roots();
    let textures = settings::milkdrop_dir().join("textures");
    let textures = textures.is_dir().then_some(textures);
    let mut library = PresetLibrary::scan(&roots, textures);
    library.extend(&settings::milkdrop_favorites());
    library
}

impl Visual {
    /// Keyed on the window existing, not on paint recency: a parked workspace
    /// doesn't repaint, and dropping its target would register a second texture.
    fn forget_closed(&mut self, live: &[u64]) {
        self.panes.retain(|id, _| live.contains(id));
        self.targets.retain(|id, _| live.contains(id));
    }

    /// What the park waits for: one window going dark shouldn't freeze another.
    fn any_lit(&self, now: Instant) -> bool {
        self.panes.values().any(|pane| pane.lit(now))
    }

    /// Under hold the park waits on this instead: lit windows need audio to change.
    fn any_playing(&self, now: Instant) -> bool {
        self.panes
            .values()
            .any(|pane| pane.playing && pane.fresh(now))
    }

    fn wanted_size(&self, now: Instant) -> Option<(u32, u32)> {
        let mut side: Option<(u32, u32)> = None;
        for pane in self.panes.values().filter(|pane| pane.lit(now)) {
            side = Some(match side {
                Some((w, h)) => (w.max(pane.size.0), h.max(pane.size.1)),
                None => pane.size,
            });
        }
        side
    }
}

/// A lock rather than a global: the paint holds `&mut App` and needs this at
/// the same time, the Milkdrop panel's shape.
static VISUAL: Mutex<Option<Visual>> = Mutex::new(None);

fn visual() -> MutexGuard<'static, Option<Visual>> {
    let mut guard = VISUAL.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Visual::default());
    }
    guard
}

/// So nothing spawns a second worker after the quit hook took the first.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The static is never dropped, so without this the worker keeps issuing GL
/// calls while glibc runs Mesa's exit handlers, a segfault in the driver. The
/// quit hook stops it with [`Engine::stop`] and waits; `rox_milkdrop`'s exit
/// guard is only the backstop.
pub(crate) fn take_engine() -> Option<Engine> {
    QUITTING.store(true, Ordering::Relaxed);
    let mut guard = VISUAL.lock().ok()?;
    let visual = guard.as_mut()?;
    visual.parked = true;
    visual.engine.take()
}

/// Cheap enough for a hot path: one read lock on the settings cache.
pub(crate) fn enabled() -> bool {
    settings::backdrop_visual().enabled
}

pub(crate) fn error() -> Option<String> {
    let mut guard = visual();
    let visual = guard.as_mut()?;
    if let Some(error) = visual.error.clone() {
        return Some(error);
    }
    match visual.engine.as_ref().map(Engine::status) {
        Some(Status::Failed(message)) => Some(message),
        _ => None,
    }
}

pub(crate) fn current_preset() -> Option<PathBuf> {
    visual().as_ref().and_then(|visual| visual.current.clone())
}

fn library(visual: &mut Visual) -> &PresetLibrary {
    if visual.library.is_none() {
        visual.library = Some(scan_library());
    }
    visual.library.as_ref().expect("just scanned")
}

pub(crate) fn presets() -> Vec<PathBuf> {
    let mut guard = visual();
    let visual = guard.as_mut().expect("initialised on first lock");
    library(visual).presets().to_vec()
}

/// A unit: the backdrop is a static, read fresh on every call.
pub(crate) struct BackdropHost;

impl PresetHost for BackdropHost {
    fn title(&self, _cx: &App) -> SharedString {
        rox_i18n::t!("milkdrop-picker-backdrop")
    }

    fn presets(&self, _cx: &mut App) -> Vec<PathBuf> {
        presets()
    }

    fn current(&self, _cx: &App) -> Option<PathBuf> {
        current_preset()
    }

    fn pick(&self, path: PathBuf, cx: &mut App) {
        pick_preset(path, cx);
    }

    fn random(&self, cx: &mut App) {
        random_preset(cx);
    }
}

/// Works with nothing playing: a parked worker takes the load, and an unstarted
/// one gets it from the config. The readout moves now, not on the worker's answer.
pub(crate) fn pick_preset(path: PathBuf, cx: &mut App) {
    let mut config = settings::backdrop_visual();
    config.preset = Some(path.clone());
    settings::note_backdrop_visual(config.clone());
    Settings::update(move |s| s.backdrop_visual = config);
    {
        let mut guard = visual();
        if let Some(visual) = guard.as_mut() {
            visual.current = Some(path.clone());
            if let Some(engine) = visual.engine.as_ref() {
                engine.send(Command::LoadPreset { path, smooth: true });
            }
        }
    }
    wake(cx);
}

/// Picked here so it works before the worker exists.
pub(crate) fn random_preset(cx: &mut App) {
    let picked = {
        let mut guard = visual();
        let visual = guard.as_mut().expect("initialised on first lock");
        let config = settings::backdrop_visual();
        let current = visual.current.clone();
        let rotation = rotation(visual, &config);
        let library = library(visual);
        let mut indices = library.rotation_indices(&rotation);
        if indices.is_empty() {
            indices = library.rotation_indices(&Rotation::All);
        }
        let except = current
            .as_deref()
            .and_then(|path| library.index_of(path))
            .and_then(|index| indices.iter().position(|i| *i == index));
        Shuffle::new()
            .pick(indices.len(), except)
            .map(|slot| library.presets()[indices[slot]].clone())
    };
    if let Some(path) = picked {
        pick_preset(path, cx);
    }
}

/// The pump notifies sixty times a second; only the flip is worth waking windows for.
pub(crate) fn note_playing(player: gpui::EntityId, playing: bool) -> bool {
    let mut guard = visual();
    let Some(visual) = guard.as_mut() else {
        return false;
    };
    visual.playing.insert(player, playing) != Some(playing)
}

/// Restarts a parked paint, which otherwise sustains itself. Deferred: the
/// callers are mid-update.
pub(crate) fn wake(cx: &mut App) {
    cx.defer(|cx| {
        for handle in cx.windows() {
            handle.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// The canvas over the blurred cover. `allowed` is the backdrop gate; losing
/// it fades out and hands the texture back. A window still holding a texture
/// gets a canvas even when off, since only its own paint can release it.
pub(crate) fn layer(window: &Window, allowed: bool) -> Option<AnyElement> {
    let id = window.window_handle().window_id().as_u64();
    let holding = visual()
        .as_ref()
        .is_some_and(|visual| visual.targets.contains_key(&id));
    if (!enabled() || !allowed) && !holding {
        return None;
    }
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| paint(bounds, window, cx, allowed),
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

/// Device pixels times the config's scale, clamped. Pure, for testing.
fn render_size(width: f32, height: f32, scale_factor: f32, scale: f32) -> (u32, u32) {
    let side = |logical: f32| {
        let device = logical * scale_factor * scale;
        // NaN and negatives land on the floor: f32 `max` skips a NaN and the cast saturates.
        (device.round().max(0.0) as u32).clamp(MIN_SIDE, MAX_SIDE)
    };
    (side(width), side(height))
}

fn weight(strength: f32, opacity: f32) -> f32 {
    // `max` after each clamp guards NaN, which clamp passes through and which
    // would draw nothing from the uniform block.
    let strength = strength.clamp(0.0, 1.0).max(0.0);
    (strength * fade::mix(opacity)).clamp(0.0, 1.0).max(0.0)
}

fn paint(bounds: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut App, allowed: bool) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    // Past the quit hook: never spawn a GL context in a dying process.
    if QUITTING.load(Ordering::Relaxed) {
        return;
    }
    let config = settings::backdrop_visual();
    let id = window.window_handle().window_id().as_u64();
    let now = Instant::now();
    let mut guard = visual();
    let Some(visual) = guard.as_mut() else {
        return;
    };

    // An unregistered window reads as nothing playing rather than borrowing audio.
    let player = surface::window_player(window, cx);
    let playing = player
        .as_ref()
        .is_some_and(|player| player.read(cx).is_playing());

    let wants = render_size(
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        window.scale_factor(),
        config.scale,
    );
    // Under hold a pane stays lit through a pause with its last frame; under
    // fade it follows the audio on the config's clock.
    let hold = !config.fade;
    let duration = if hold {
        FADE
    } else {
        Duration::from_secs_f32(
            config
                .fade_secs
                .clamp(0.0, settings::BACKDROP_VISUAL_FADE_MAX),
        )
    };
    let pane = visual.panes.entry(id).or_insert_with(|| Pane {
        size: wants,
        at: now,
        fade: Fade::settled(0.0),
        duration,
        playing: false,
        held: false,
    });
    pane.size = wants;
    pane.at = now;
    pane.duration = duration;
    pane.playing = playing;
    pane.held |= playing;
    let lit = config.enabled && allowed && (playing || (hold && pane.held));
    pane.fade.retarget(if lit { 1.0 } else { 0.0 }, duration);
    let opacity = pane.fade.opacity(duration);
    let running = pane.fade.running(duration);

    // Drain even while dark, or the Appearance page's readout never moves.
    if let Some(engine) = visual.engine.clone() {
        drain(visual, &engine, &config, cx);
    }

    if opacity <= 0.0 && !running {
        let live: Vec<u64> = cx
            .windows()
            .iter()
            .map(|h| h.window_id().as_u64())
            .collect();
        visual.forget_closed(&live);
        // Release only when the layer is gone for good; a stop keeps the texture
        // so the next play doesn't re-register and recompile.
        if !config.enabled || !allowed {
            release(visual, id, window);
        }
        // Nothing lit anywhere: park rather than tear down, so resuming skips a GL
        // context. Switched off everywhere, drop the engine.
        if !visual.any_lit(now) {
            if !visual.parked && visual.engine.is_some() {
                visual.parked = true;
                if let Some(engine) = visual.engine.as_ref() {
                    engine.send(Command::Pause);
                }
            }
            if !config.enabled && visual.targets.is_empty() {
                visual.engine = None;
                visual.driving = None;
                visual.size = None;
                visual.pending = None;
                visual.error = None;
            }
        }
        return;
    }

    let want = visual.wanted_size(now).unwrap_or((MIN_SIDE, MIN_SIDE));
    if visual.size == Some(want) {
        visual.pending = None;
    } else if visual.size.is_none() || visual.pending == Some(want) {
        visual.pending = None;
        visual.size = Some(want);
        if let Some(engine) = visual.engine.as_ref() {
            engine.send(Command::Resize {
                width: want.0,
                height: want.1,
            });
        }
    } else {
        // First sighting: request a frame so the second one arrives.
        visual.pending = Some(want);
        window.request_animation_frame();
    }
    let Some(size) = visual.size else {
        return;
    };

    // A new player only takes over from parked: a new feed means a new GL
    // context, which mid-track would show as a hole.
    if let Some(player) = player.as_ref() {
        let swap = visual
            .driving
            .is_none_or(|driving| driving != player.entity_id() && visual.parked);
        if visual.engine.is_none() || swap {
            start(visual, player, size, cx);
        }
    }
    // Under fade the worker feeds the fade-out too. Under hold the still frame
    // stays up, so the worker only runs while some window has audio.
    let awake = !hold || visual.any_playing(now);
    if awake && visual.parked {
        visual.parked = false;
        if let Some(engine) = visual.engine.as_ref() {
            engine.send(Command::Resume);
        }
    } else if !awake && !visual.parked && visual.engine.is_some() {
        visual.parked = true;
        if let Some(engine) = visual.engine.as_ref() {
            engine.send(Command::Pause);
        }
    }
    let Some(engine) = visual.engine.clone() else {
        return;
    };
    sync(visual, &engine, &config);

    let fits = visual
        .targets
        .get(&id)
        .is_some_and(|target| (target.width, target.height) == size);
    if !fits {
        release(visual, id, window);
        match window.register_dynamic_texture(size.0, size.1) {
            Ok(texture) => {
                visual.error = None;
                visual.targets.insert(
                    id,
                    Target {
                        texture,
                        width: size.0,
                        height: size.1,
                        chain: None,
                        last_seq: 0,
                    },
                );
            }
            Err(message) => {
                visual.error = Some(message);
                return;
            }
        }
    }
    let Some(target) = visual.targets.get_mut(&id) else {
        return;
    };

    // A frame from before a resize is dropped, but its seq still advances.
    if let Some(frame) = engine.frame_after(target.last_seq) {
        target.last_seq = frame.seq;
        if frame.width == target.width
            && frame.height == target.height
            && let Err(message) = window.update_user_texture(target.texture, frame.rgba8)
        {
            visual.error = Some(message);
            return;
        }
    }

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
                visual.error = None;
            }
            Err(message) => {
                visual.error = Some(message);
                return;
            }
        }
    }
    let Some(shader) = visual.targets.get(&id).and_then(|target| target.chain) else {
        return;
    };

    let meta = surface::meta_slots(window, cx);
    let mut signals = [0.0f32; 16];
    signals[grade::SLOT_FADE] = weight(config.strength, opacity);
    signals[SLOT_FLIP_H] = if config.flip_horizontal { 1.0 } else { 0.0 };
    signals[SLOT_FLIP_V] = if config.flip_vertical { 1.0 } else { 0.0 };
    // The app-wide theme, not a panel's: this sits under every panel. The
    // resolved palette carries the cover tint when song theming is on.
    let theme = palette::resolved();
    let cover = player
        .as_ref()
        .and_then(|player| palette::seed(player.entity_id()))
        .and_then(|seed| seed.primary);
    Grade::new(
        grade_mode(config.color),
        palette::mode() == Mode::Light,
        theme.bg_root,
        theme.accent,
        cover,
    )
    .write(&mut signals);
    // A still frame over a parked worker needs no frame requests; the play flip
    // and settings writes wake every window anyway.
    let still = visual.parked && !running && visual.pending.is_none();
    drop(guard);
    window.paint_screen_shader(bounds, shader, INSTANCE, signals, meta);
    if !still {
        window.request_animation_frame();
    }
}

/// Only the window's own registry can free it, so this runs from the paint.
fn release(visual: &mut Visual, id: u64, window: &mut Window) {
    if let Some(old) = visual.targets.remove(&id) {
        window.release_user_texture(old.texture);
    }
}

fn start(visual: &mut Visual, player: &Entity<Player>, size: (u32, u32), cx: &App) {
    if visual.library.is_none() {
        visual.library = Some(scan_library());
    }
    let library = visual.library.clone().expect("just scanned");
    let config = settings::backdrop_visual();
    // Dropping joins the old thread, so two contexts are never alive at once.
    visual.engine = None;
    // The last preset goes in with the spawn so the worker doesn't shuffle first.
    let engine = Engine::spawn(EngineOptions {
        feed: player.read(cx).feed(),
        library,
        preset: config.preset.clone().filter(|path| path.is_file()),
        fps: config.fps,
        width: size.0,
        height: size.1,
    });
    engine.send(Command::SetPresetDuration(config.duration_secs));
    engine.send(Command::SetBeatSensitivity(config.beat_sensitivity));
    engine.send(Command::SetHardCut(config.hard_cuts));
    engine.send(Command::SetLocked(config.locked));
    let rotation = rotation(visual, &config);
    engine.send(Command::SetRotation(rotation));
    visual.applied = Some(Applied::of(&config));
    visual.engine = Some(engine);
    visual.driving = Some(player.entity_id());
    visual.parked = false;
    visual.current = None;
    for target in visual.targets.values_mut() {
        target.last_seq = 0;
    }
}

fn sync(visual: &mut Visual, engine: &Engine, config: &BackdropVisualConfig) {
    let want = Applied::of(config);
    let Some(had) = visual.applied.clone() else {
        visual.applied = Some(want);
        return;
    };
    if had == want {
        return;
    }
    if had.locked != want.locked {
        engine.send(Command::SetLocked(want.locked));
    }
    if had.duration_secs != want.duration_secs {
        engine.send(Command::SetPresetDuration(want.duration_secs));
    }
    if had.fps != want.fps {
        engine.send(Command::SetFps(want.fps));
    }
    if had.beat_sensitivity != want.beat_sensitivity {
        engine.send(Command::SetBeatSensitivity(want.beat_sensitivity));
    }
    if had.hard_cuts != want.hard_cuts {
        engine.send(Command::SetHardCut(want.hard_cuts));
    }
    if had.lists_gen != want.lists_gen {
        // Only a folder edit rescans. A new favorite folds into the held library:
        // this runs on the paint, and a pack walk per star would hang every window.
        let library = library(visual);
        let before = library.presets().len();
        let mut grown = if library.roots() != settings::milkdrop_scan_roots().as_slice() {
            scan_library()
        } else {
            library.clone()
        };
        grown.extend(&settings::milkdrop_favorites());
        if grown.presets().len() != before || grown.roots() != library.roots() {
            set_library(visual, grown.clone());
            let rotation = rotation(visual, config);
            engine.send(Command::SetLibrary {
                library: grown,
                rotation,
            });
        } else if config.favorites_only {
            let rotation = rotation(visual, config);
            engine.send(Command::SetRotation(rotation));
        }
    } else if had.favorites_only != want.favorites_only
        || had.rotation_folder != want.rotation_folder
    {
        let rotation = rotation(visual, config);
        engine.send(Command::SetRotation(rotation));
    }
    visual.applied = Some(want);
}

/// A preset switch wakes every window once for the Appearance readout. While
/// locked, the preset is also written to settings so a restart lands on it.
fn drain(visual: &mut Visual, engine: &Engine, config: &BackdropVisualConfig, cx: &mut App) {
    for event in engine.take_events() {
        match event {
            Event::PresetChanged(path) => {
                if config.locked && config.preset.as_ref() != Some(&path) {
                    let mut noted = config.clone();
                    noted.preset = Some(path.clone());
                    settings::note_backdrop_visual(noted.clone());
                    cx.defer(move |_| Settings::update(move |s| s.backdrop_visual = noted));
                }
                rox_panels::milkdrop::thumbnails().loaded(&path);
                visual.current = Some(path);
                wake(cx);
            }
            Event::PresetFailed { path, message } => {
                log::warn!(
                    "milkdrop backdrop preset {} failed: {message}",
                    path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_render_size_clamps_both_sides() {
        assert_eq!(render_size(1920., 1080., 1.0, 0.5), (960, 540));
        assert_eq!(render_size(4., 4., 1.0, 0.5), (MIN_SIDE, MIN_SIDE));
        assert_eq!(render_size(9000., 9000., 2.0, 1.0), (MAX_SIDE, MAX_SIDE));
        assert_eq!(render_size(f32::NAN, -5., 1.0, 0.5), (MIN_SIDE, MIN_SIDE));
    }

    #[test]
    fn the_frame_pass_compiles_with_the_grade_in_scope() {
        surface::validate_frame_pass(&grade::wgsl(FRAME_WGSL), &["frame"])
            .expect("the backdrop's pass validates");
    }

    #[test]
    fn the_weight_is_the_strength_scaled_by_the_fade() {
        assert_eq!(weight(0.0, 1.0), 0.0);
        assert_eq!(weight(1.0, 1.0), 1.0);
        assert_eq!(weight(0.5, 0.0), 0.0);
        assert!(weight(0.5, 0.5) < 0.25);
        assert!(weight(f32::NAN, f32::NAN).is_finite());
    }

    fn lit_pane(size: (u32, u32), now: Instant) -> Pane {
        Pane {
            size,
            at: now,
            fade: Fade::settled(1.0),
            duration: FADE,
            playing: true,
            held: true,
        }
    }

    #[test]
    fn the_shared_size_is_the_per_axis_max_of_the_lit_windows() {
        let now = Instant::now();
        let mut visual = Visual::default();
        visual.panes.insert(1, lit_pane((1920, 600), now));
        visual.panes.insert(2, lit_pane((800, 1200), now));
        assert_eq!(visual.wanted_size(now), Some((1920, 1200)));
        visual.panes.get_mut(&1).expect("inserted").at = now - WINDOW_STALE * 2;
        assert_eq!(visual.wanted_size(now), Some((800, 1200)));
        visual.forget_closed(&[2]);
        assert!(!visual.panes.contains_key(&1));
        assert!(visual.panes.contains_key(&2));
    }

    #[test]
    fn a_dark_window_neither_sizes_the_render_nor_holds_the_worker_awake() {
        let now = Instant::now();
        let mut visual = Visual::default();
        visual.panes.insert(1, lit_pane((1920, 1080), now));
        visual.panes.insert(
            2,
            Pane {
                size: (3840, 2160),
                at: now,
                fade: Fade::settled(0.0),
                duration: FADE,
                playing: false,
                held: false,
            },
        );
        assert_eq!(visual.wanted_size(now), Some((1920, 1080)));
        assert!(visual.any_lit(now));
        visual.panes.get_mut(&1).expect("inserted").fade = Fade::settled(0.0);
        assert!(!visual.any_lit(now));
        assert_eq!(visual.wanted_size(now), None);
    }

    #[test]
    fn under_hold_the_park_waits_on_audio_rather_than_light() {
        let now = Instant::now();
        let mut visual = Visual::default();
        visual.panes.insert(1, lit_pane((1920, 1080), now));
        assert!(visual.any_playing(now));
        visual.panes.get_mut(&1).expect("inserted").playing = false;
        assert!(visual.any_lit(now));
        assert!(!visual.any_playing(now));
        visual
            .panes
            .insert(2, lit_pane((800, 600), now - WINDOW_STALE * 2));
        assert!(!visual.any_playing(now));
    }
}
