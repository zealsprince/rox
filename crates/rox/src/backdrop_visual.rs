//! The Milkdrop visual behind the whole app: one engine, every window.
//!
//! ADR 10's backdrop is the playing track's art, downscaled and blurred once
//! per track change and stretched behind the window. This adds a second
//! layer straight over it: a live MilkDrop frame, composited at a strength
//! the user sets, while something is playing. Zero strength is the art
//! alone, so the whole feature folds back to what ADR 10 already does.
//!
//! ## One engine
//!
//! [`rox_milkdrop::Engine`] renders on a thread of its own into a private GL
//! context and reads the pixels back, and the readback is the bill: about
//! 2.5 ms a frame at 1080p on this machine. Nothing about that cost gets
//! cheaper by paying it twice, so there is exactly one engine for the app,
//! it renders at one size, and every window uploads the same frame into a
//! texture of its own. Textures and compiled chains belong to the window
//! that handed them out, so that part can't be shared and isn't.
//!
//! The one size is the per-axis maximum over the windows currently painting
//! the layer, times the render scale. Each window then draws it cover-fit,
//! filling its bounds and cropping the overflow rather than letterboxing:
//! a backdrop with bars down the side isn't a backdrop. Taking the max
//! rather than, say, the frontmost window's size is what makes cover-fit
//! never upscale in any of them, since a source at least as wide and at
//! least as tall as a window covers it at a scale of one or less. A small
//! child window therefore reads a crop out of the middle of the workspace's
//! frame, which is the right answer for a defocused wash and the wrong one
//! for anything you'd look at.
//!
//! ## Composite
//!
//! The pass writes `vec4(rgb * a, a)` with `a` the strength times the fade.
//! The region pipeline blends premultiplied-over, so that lands the result
//! exactly `a` of the way from whatever is already on the screen (the
//! blurred cover) to the frame. The strength control is the alpha; there's
//! no mix to invent, and nothing has to guess what the layer under it
//! painted.
//!
//! ## Cost
//!
//! This runs the entire time audio is playing, which a panel doesn't. The
//! defaults are chosen against that: half scale (a quarter of the pixels,
//! so roughly a quarter of the readback) at 30 fps, which is about an
//! eighth of what a full-size 60 fps panel costs. It parks the worker at
//! the bottom of the fade, so a stopped player costs one atomic load per
//! window per frame and nothing else.
//!
//! ADR 28 is the decision the engine sits under; the layering here is
//! ADR 10's.

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

/// The smallest and largest render each side gets. The floor keeps a window
/// dragged down to a sliver from asking projectM for a 4x1 framebuffer; the
/// ceiling keeps a maximised window on a 5K display from turning the
/// readback into a 30 MB memcpy per frame.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 4096;

/// How long the visual takes to come up and go away under hold, where the
/// only transitions are the switch and the first play: the play-state fade
/// under the fade setting runs on the config's own time. Fixed because a
/// backdrop that takes its own sweet time to appear reads as a bug.
const FADE: Duration = Duration::from_millis(700);

/// How long a window's stated size counts after its last paint. A window
/// that stopped drawing the layer, because it was gated off or hidden,
/// stops reporting, and its size must stop inflating the render the other
/// windows share.
const WINDOW_STALE: Duration = Duration::from_secs(1);

/// Keys the region's scratch texture. Everywhere else in rox that's a view's
/// entity id; this layer has no view, so it takes a value the entity
/// counter will never reach. The only requirement is that no two live
/// regions in one window share it.
const INSTANCE: u64 = u64::MAX;

/// The one pass: sample the frame, cover-fit it into the window, run it
/// through the theme grade, and carry the strength as premultiplied alpha.
///
/// Cover rather than the Milkdrop panel's aspect-fit, because a backdrop
/// has to reach every corner. The source is never smaller than the window
/// on either axis (see the module header on how the size is picked), so the
/// fit only ever crops.
///
/// The grade is what makes the light theme work at all: a frame drawn on
/// black composited at a third over a pale cover wash is a grey smear, and
/// the same frame with its lightness flipped is the pale wash with the
/// preset's shapes moving in it. The maths is [`rox_panel_kit::grade`]'s,
/// prepended to this source; the slots it reads are filled below.
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

/// The signal slots the flips ride in, past the grade's, the same two
/// the Milkdrop panel's shader reads.
const SLOT_FLIP_H: usize = 14;
const SLOT_FLIP_V: usize = 15;

/// One window's texture and compiled chain. Both belong to the window that
/// handed them out, so there's one of these per window painting the layer.
struct Target {
    texture: UserTextureId,
    width: u32,
    height: u32,
    /// The one-pass chain binding `texture`. Registered on the first paint
    /// after the texture is made, and dropped with it on a resize.
    chain: Option<UserShaderId>,
    /// The newest frame this window has uploaded. The engine hands back
    /// anything past it and nothing else, so a UI frame with no new render
    /// behind it costs one atomic load.
    last_seq: u64,
}

/// One window's slice of the layer: how big it wants the render, when it
/// last said so, and where its own fade stands.
///
/// The fade is per window rather than shared because the thing it follows
/// is per window. Each window's backdrop answers to its own workspace's
/// player, and the gate that keeps the visual out of child windows is per
/// window too, so one settings window fading out must not take the
/// workspace's visual with it.
struct Pane {
    size: (u32, u32),
    at: Instant,
    fade: Fade,
    /// How long this pane's fade runs: the config's under fade, [`FADE`]
    /// under hold. Kept here so the lit test reads the same clock the
    /// paint retargeted with.
    duration: Duration,
    /// Whether this window's player was playing at the last paint. What
    /// the worker's park keys on under hold, where a lit window isn't a
    /// reason to render.
    playing: bool,
    /// Whether this window has ever seen its player play. Under hold a
    /// pane that never has holds nothing, so it stays dark rather than
    /// pinning an empty texture over the cover at full strength.
    held: bool,
}

impl Pane {
    /// Whether this window has anything on screen, fading in or out
    /// included. A dark pane costs the engine nothing and doesn't count
    /// toward the shared render size.
    fn lit(&self, now: Instant) -> bool {
        (self.fade.to > 0.0 || self.fade.running(self.duration)) && self.fresh(now)
    }

    /// Whether this window painted recently enough to have a say.
    fn fresh(&self, now: Instant) -> bool {
        now.duration_since(self.at) < WINDOW_STALE
    }
}

/// Everything the layer owns, shared by every window that paints it.
///
/// A global rather than an entity because the paint that drives it runs
/// from the backdrop service's shade hook, which is handed a window and an
/// app and nothing else. That's the same reason the backdrop shader's
/// compile message lives in a static beside the workspace.
#[derive(Default)]
struct Visual {
    engine: Option<Engine>,
    /// The player whose audio the running engine was spawned against. The
    /// feed is fixed at spawn, so switching players means a new context,
    /// which is only ever done from parked.
    driving: Option<gpui::EntityId>,
    /// Every window painting the layer, by window id.
    panes: HashMap<u64, Pane>,
    targets: HashMap<u64, Target>,
    /// The size the engine renders at, and a size seen once and not yet
    /// acted on: a second sighting is what commits it, so dragging a window
    /// edge doesn't reallocate the framebuffer on every pixel of the drag.
    size: Option<(u32, u32)>,
    pending: Option<(u32, u32)>,
    parked: bool,
    /// What the window or the worker last said went wrong, for the settings
    /// page's readout.
    error: Option<String>,
    /// The presets, scanned once when the layer first runs. A backdrop has
    /// no settings page of its own to press Rescan on; it picks up new
    /// packs the next time the app starts, which is the same deal the
    /// panel's worker makes with its own snapshot.
    library: Option<PresetLibrary>,
    /// The play state each workspace's player was last seen in, so a pump
    /// tick that says the same thing as the last one costs nothing.
    playing: HashMap<gpui::EntityId, bool>,
    /// The preset the worker last said it switched to, for the settings
    /// page's readout and the favorite star beside it.
    current: Option<PathBuf>,
    /// The knobs the running worker was last told, so a paint can tell
    /// whether the settings moved with one compare rather than re-sending
    /// every command every frame.
    applied: Option<Applied>,
    /// Every folder in the scan that holds presets, and the rotation
    /// picker's row for each, worked out once per scan. See
    /// [`rotation_folders`].
    folders: Option<Vec<PathBuf>>,
    folder_options: Option<Arc<Vec<(String, SharedString)>>>,
}

/// The slice of the config that goes to the worker as commands, plus the
/// favorites edit it was resolved against.
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

/// What the config's rotation means to the engine: the favorites while
/// the switch is on, the picked folder where the scan holds it, the
/// whole library otherwise. The same reading the panel makes of its own
/// config.
fn rotation(visual: &mut Visual, config: &BackdropVisualConfig) -> Rotation {
    if config.favorites_only {
        return Rotation::Set(settings::milkdrop_favorites());
    }
    match rotation_dir(visual, config) {
        Some(folder) => Rotation::Folder(folder),
        None => Rotation::All,
    }
}

/// The folder the config's rotation names on this machine, found in the
/// scan by its path under a root and then by its name. None with no
/// pick, or a pick this scan doesn't hold, which rotates everything the
/// way a deleted folder always did.
fn rotation_dir(visual: &mut Visual, config: &BackdropVisualConfig) -> Option<PathBuf> {
    let relative = config.rotation_folder.as_deref()?;
    let roots = library(visual).roots().to_vec();
    find_folder_by_relative(folders(visual), &roots, relative)
}

/// The scan's folders, worked out once per scan.
fn folders(visual: &mut Visual) -> &Vec<PathBuf> {
    if visual.folders.is_none() {
        visual.folders = Some(library(visual).folders());
    }
    visual.folders.as_ref().expect("just listed")
}

/// Replace the held library, and with it everything worked out of it.
fn set_library(visual: &mut Visual, library: PresetLibrary) {
    visual.library = Some(library);
    visual.folders = None;
    visual.folder_options = None;
}

/// What the Appearance page's rotation picker offers past All and
/// Favorites: every folder in the scan that holds presets, keyed by its
/// path under its root and labelled the same way. Built once per scan and
/// shared from there.
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

/// The rotation picker's reading of the config, and what it writes back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BackdropRotation {
    All,
    Favorites,
    /// A folder by its path under a scan root.
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

    /// Write the pick into the config: All clears the folder too, since a
    /// folder that comes back when Favorites is switched off is the
    /// panel's behaviour, but All is a choice to rotate everything.
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

/// The scan the backdrop walks: the same folders every panel walks, plus
/// whatever is starred.
fn scan_library() -> PresetLibrary {
    let roots = settings::milkdrop_scan_roots();
    let textures = settings::milkdrop_dir().join("textures");
    let textures = textures.is_dir().then_some(textures);
    let mut library = PresetLibrary::scan(&roots, textures);
    library.extend(&settings::milkdrop_favorites());
    library
}

impl Visual {
    /// Forget the windows that have closed.
    ///
    /// Keyed on the window still existing rather than on how long since it
    /// last painted: a workspace with the visual parked doesn't repaint at
    /// all, and dropping its target out from under it would have its next
    /// paint register a second texture in the same window. The texture
    /// itself dies with the window, the way a registered image does.
    fn forget_closed(&mut self, live: &[u64]) {
        self.panes.retain(|id, _| live.contains(id));
        self.targets.retain(|id, _| live.contains(id));
    }

    /// Whether anything is on screen anywhere. What the worker's park waits
    /// for: one window's fade bottoming out isn't a reason to freeze the
    /// picture in another.
    fn any_lit(&self, now: Instant) -> bool {
        self.panes.values().any(|pane| pane.lit(now))
    }

    /// Whether any window's player is playing. Under hold this is what the
    /// park waits for instead: every window stays lit with its held frame,
    /// and the worker only needs to run while one of them has audio.
    fn any_playing(&self, now: Instant) -> bool {
        self.panes
            .values()
            .any(|pane| pane.playing && pane.fresh(now))
    }

    /// The sizes the lit windows want, folded to the per-axis maximum.
    /// None while nothing is showing.
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

/// The layer's own lock rather than a gpui global.
///
/// The paint runs with a `&mut App` already in hand and has to touch this
/// state at the same time, which a global borrow can't do. The Milkdrop
/// panel keeps its own paint state behind a mutex for exactly that reason;
/// this is the same shape, one level up.
static VISUAL: Mutex<Option<Visual>> = Mutex::new(None);

fn visual() -> MutexGuard<'static, Option<Visual>> {
    let mut guard = VISUAL.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Visual::default());
    }
    guard
}

/// Latched by [`take_engine`] on the way out, so nothing starts a second
/// worker after the quit hook has taken the first one down.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Take the running engine out for shutdown. `None` if the layer never
/// started one.
///
/// A process static is never dropped, so on quit the worker in here outlives
/// `main` and goes on issuing GL calls while glibc runs Mesa's own exit
/// handlers underneath it, which is a segfault in the driver's format table.
/// The quit hook in `main` takes the engine out here and drops it, which
/// hangs up the worker's channel and waits for the thread. `rox_milkdrop`'s
/// exit guard is the backstop under that, not the mechanism.
pub(crate) fn take_engine() -> Option<Engine> {
    QUITTING.store(true, Ordering::Relaxed);
    let mut guard = VISUAL.lock().ok()?;
    let visual = guard.as_mut()?;
    visual.parked = true;
    visual.engine.take()
}

/// Whether the visual is switched on. Cheap enough for a hot path: one
/// read lock on the settings cache.
pub(crate) fn enabled() -> bool {
    settings::backdrop_visual().enabled
}

/// What the layer last failed at, for the settings page's banner. None is
/// a clean run or nothing running.
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

/// The preset on screen, for the Appearance page's readout. None before
/// the first switch or while nothing runs.
pub(crate) fn current_preset() -> Option<PathBuf> {
    visual().as_ref().and_then(|visual| visual.current.clone())
}

/// The library the backdrop walks, scanned on first ask and held.
fn library(visual: &mut Visual) -> &PresetLibrary {
    if visual.library.is_none() {
        visual.library = Some(scan_library());
    }
    visual.library.as_ref().expect("just scanned")
}

/// Every preset the backdrop's library holds, for the picker. Scanned on
/// first ask like everything else here.
pub(crate) fn presets() -> Vec<PathBuf> {
    let mut guard = visual();
    let visual = guard.as_mut().expect("initialised on first lock");
    library(visual).presets().to_vec()
}

/// The backdrop as the preset picker sees it. A unit: the backdrop is a
/// static, so there's nothing to hold, and every call reads it fresh.
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

/// Put a preset up, from the Appearance page. Works with nothing
/// playing: the worker takes the load while parked, and a worker that
/// hasn't started yet gets the pick at start, since it's written to the
/// config either way. The readout moves now rather than on the worker's
/// answer, so a pick with the visual off still reads back.
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

/// A random pick from the rotation, chosen here rather than by the worker
/// so it works before the worker exists. Avoids the preset that's up.
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

/// Whether a player's play state moved since the last time it was noted.
/// The workspace's pump observer fires sixty times a second, and only the
/// flip is worth waking every window in the app for.
pub(crate) fn note_playing(player: gpui::EntityId, playing: bool) -> bool {
    let mut guard = visual();
    let Some(visual) = guard.as_mut() else {
        return false;
    };
    visual.playing.insert(player, playing) != Some(playing)
}

/// Wake every window so a switch, a slider, or a play-state flip reaches
/// the layer. The paint sustains itself with frame requests while it's
/// running; this is what restarts it once it has parked.
///
/// Deferred, because the callers are a settings write and a player
/// notification, and neither is a safe place to reach back into the window
/// that's mid-update.
pub(crate) fn wake(cx: &mut App) {
    cx.defer(|cx| {
        for handle in cx.windows() {
            handle.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// The element the backdrop's shade hook adds: a window-filling canvas
/// drawn over the blurred cover and under everything else.
///
/// `allowed` is the app's own copy of the backdrop gate: which windows get
/// the layer at all. A window that loses it fades the visual out and hands
/// its texture back rather than cutting.
///
/// None when there's nothing to do, which is the common case: switched off
/// and this window holding no texture to give back. A window that still
/// holds one gets a canvas anyway, because releasing it is something only
/// its own paint can do.
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

/// What the engine renders at, from a window's bounds: device pixels times
/// the config's scale, clamped to what a framebuffer should ever be. Pure,
/// so the clamp is testable without a window.
fn render_size(width: f32, height: f32, scale_factor: f32, scale: f32) -> (u32, u32) {
    let side = |logical: f32| {
        let device = logical * scale_factor * scale;
        // NaN and negatives both land on the floor: `max` on f32 takes the
        // other operand when one is NaN, and the cast saturates.
        (device.round().max(0.0) as u32).clamp(MIN_SIDE, MAX_SIDE)
    };
    (side(width), side(height))
}

/// The alpha the pass writes: the user's strength scaled by where the fade
/// has got to, shaped so the fade reads even.
fn weight(strength: f32, opacity: f32) -> f32 {
    // `max` after each clamp is the NaN guard, the same one the fade
    // shaping carries: clamp passes a NaN straight through, and a NaN in
    // the uniform block is a layer that draws nothing.
    let strength = strength.clamp(0.0, 1.0).max(0.0);
    (strength * fade::mix(opacity)).clamp(0.0, 1.0).max(0.0)
}

fn paint(bounds: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut App, allowed: bool) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    // Past the quit hook there is no engine and no starting one: a paint
    // that got in after it would spawn a GL context for a process on its
    // way out.
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

    // The window's own player, off the registry every workspace fills as it
    // opens. A window nobody registered (a dialog that opened first) reads
    // as nothing playing, which fades the layer out there rather than
    // guessing at another window's audio.
    let player = surface::window_player(window, cx);
    let playing = player
        .as_ref()
        .is_some_and(|player| player.read(cx).is_playing());

    // This window's own slice: what it wants rendered, and where its fade
    // is heading. Switching the visual off, or gating this window out,
    // fades it away rather than cutting, and the teardown waits for the
    // bottom.
    let wants = render_size(
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
        window.scale_factor(),
        config.scale,
    );
    // Hold or fade is the config's call, the same switch the panel has.
    // Under hold the pane stays lit through a pause or a stop with the
    // frame it had, once it has ever had one; under fade it follows the
    // audio down and back on the config's own clock.
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
        // Dark and settled: a window that opens mid-track fades its visual
        // in rather than snapping it on.
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

    // The worker takes commands while parked, and the Appearance page can
    // send it one with nothing playing. Its answer has to be read even
    // while this window is dark, or the readout there never moves.
    if let Some(engine) = visual.engine.clone() {
        drain(visual, &engine, &config, cx);
    }

    if opacity <= 0.0 && !running {
        // Nothing on screen here, so this is the cheap moment to drop what
        // closed windows left behind rather than doing it on every frame.
        let live: Vec<u64> = cx
            .windows()
            .iter()
            .map(|h| h.window_id().as_u64())
            .collect();
        visual.forget_closed(&live);
        // Hand the texture back if this window has stopped showing the
        // layer for good; a stop with the switch still on keeps it, so the
        // next play doesn't pay to register a texture and compile a chain
        // again.
        if !config.enabled || !allowed {
            release(visual, id, window);
        }
        // Once no window is showing anything, park the worker: freeze it
        // rather than tear it down, so a resume doesn't pay for a GL
        // context. Switched off everywhere, let the engine go entirely.
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
        // A drag that came back where it started leaves a size sitting in
        // `pending` that nothing will ever commit, and the next real resize
        // to that same size would skip its own debounce.
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
        // First sighting of this size. Ask for another frame so the second
        // sighting arrives even if nothing else is moving, and keep drawing
        // at the old size meanwhile.
        visual.pending = Some(want);
        window.request_animation_frame();
    }
    let Some(size) = visual.size else {
        return;
    };

    // The engine, started on the first paint that has both a size and a
    // player to listen to. A different player only ever takes over from
    // parked: swapping the feed means a new GL context, and paying for one
    // mid-track would show as a hole in the visual.
    if let Some(player) = player.as_ref() {
        let swap = visual
            .driving
            .is_none_or(|driving| driving != player.entity_id() && visual.parked);
        if visual.engine.is_none() || swap {
            start(visual, player, size, cx);
        }
    }
    // Under fade, being here means something is still on screen and the
    // worker has to feed it, fade-out included: parking early would
    // freeze the picture the fade is taking away. Under hold the screen
    // stays lit with a still frame, so the worker only runs while some
    // window's player has audio, and the frame it left is what every
    // window keeps sampling.
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

    // This window's texture, remade whenever the shared render size moves.
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

    // The newest render, if there's one past what this window already has.
    // A frame from before a resize is dropped rather than stretched; its
    // seq still moves on, so it's dropped once and not re-fetched.
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

    // A chain that binds an asset can only run as a screen pass, so this is
    // always the region branch, the same as the Milkdrop panel's.
    let meta = surface::meta_slots(window, cx);
    let mut signals = [0.0f32; 16];
    signals[grade::SLOT_FADE] = weight(config.strength, opacity);
    signals[SLOT_FLIP_H] = if config.flip_horizontal { 1.0 } else { 0.0 };
    signals[SLOT_FLIP_V] = if config.flip_vertical { 1.0 } else { 0.0 };
    // The grade reads the app-wide theme rather than any panel's scope:
    // this layer sits under every panel, so it's the window's theme it
    // has to agree with. The resolved palette carries the cover tint when
    // song theming is on, which is how the palette ramp follows the art.
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
    // A held frame over a parked worker has nothing new coming: the next
    // paint is the play flip's or a settings write's to ask for, and both
    // wake every window. Asking every frame would spin the paint on a
    // still texture for as long as the music sits paused.
    let still = visual.parked && !running && visual.pending.is_none();
    // Everything this layer owns has been read; the record and the frame
    // request below don't need it, so the lock goes back first.
    drop(guard);
    window.paint_screen_shader(bounds, shader, INSTANCE, signals, meta);
    if !still {
        window.request_animation_frame();
    }
}

/// Give a window its texture back. Only that window's own registry can free
/// it, which is why this runs from the paint and not from a settings write.
fn release(visual: &mut Visual, id: u64, window: &mut Window) {
    if let Some(old) = visual.targets.remove(&id) {
        window.release_user_texture(old.texture);
    }
}

/// Start the worker against a player's feed. Replaces a parked engine
/// wholesale, since the feed is fixed at spawn.
fn start(visual: &mut Visual, player: &Entity<Player>, size: (u32, u32), cx: &App) {
    if visual.library.is_none() {
        visual.library = Some(scan_library());
    }
    let library = visual.library.clone().expect("just scanned");
    let config = settings::backdrop_visual();
    // Dropping the old engine joins its thread, so the two contexts are
    // never alive at once.
    visual.engine = None;
    // The preset that was picked or locked last time comes back up, and it
    // goes in with the spawn so the worker doesn't shuffle one first. An
    // unlocked backdrop moves on from it after the duration, which is
    // what unlocked means; it still starts where it was left.
    let engine = Engine::spawn(EngineOptions {
        feed: player.read(cx).feed(),
        library,
        preset: config.preset.clone().filter(|path| path.is_file()),
        fps: config.fps,
        width: size.0,
        height: size.1,
    });
    // The knobs the config carries, pushed once at startup so a restart
    // comes up the way it was left rather than at projectM's defaults.
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
    // Every window's texture now belongs to a frame stream that starts over
    // at seq zero, so nothing carries.
    for target in visual.targets.values_mut() {
        target.last_seq = 0;
    }
}

/// Push whatever moved in the settings since the worker last heard: the
/// lock, the duration, the frame rate, the favorites switch, or the
/// favorites list itself.
/// One compare per paint, and nothing goes down the channel while nothing
/// changed.
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
        // A folder edit is the one thing that earns a rescan. A new
        // favorite is folded into the library already held instead: this
        // runs on the paint, and a walk of the pack per star is a click
        // that hangs every window.
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

/// Take what the worker has to say since the last paint: which preset
/// came up, and which one wouldn't load.
///
/// A switch wakes every window once so an open Appearance page updates
/// its readout; that's one refresh every half minute while the visual
/// runs, which nothing notices. While the lock is on the preset is also
/// written to settings, so the restart lands back on it. The lock is what
/// makes that cheap: a locked backdrop only changes preset when someone
/// asks it to.
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
        // A sliver of a window still gets a framebuffer worth allocating.
        assert_eq!(render_size(4., 4., 1.0, 0.5), (MIN_SIDE, MIN_SIDE));
        // And a wall of a window doesn't get a 30 MB readback.
        assert_eq!(render_size(9000., 9000., 2.0, 1.0), (MAX_SIDE, MAX_SIDE));
        // Nonsense in a hand-edited file lands on the floor, not a panic.
        assert_eq!(render_size(f32::NAN, -5., 1.0, 0.5), (MIN_SIDE, MIN_SIDE));
    }

    /// The pass only ever compiles inside a window, so this compiles it
    /// against the same template the window composes it into.
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
        // The shaping only bends the middle, so a half-strength layer half
        // way through its fade sits under a quarter.
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
        // Cover-fit never upscales in either window, which is the whole
        // point of taking the max on each axis rather than the larger area.
        assert_eq!(visual.wanted_size(now), Some((1920, 1200)));
        // A window that stopped painting stops counting.
        visual.panes.get_mut(&1).expect("inserted").at = now - WINDOW_STALE * 2;
        assert_eq!(visual.wanted_size(now), Some((800, 1200)));
        // And a window that closed is forgotten outright, entry and all.
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
        // The settings window sitting there faded out doesn't get to ask
        // for a 4K framebuffer.
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
        // The window's player pausing leaves its frame lit and held, and
        // that alone is no reason to keep rendering.
        visual.panes.get_mut(&1).expect("inserted").playing = false;
        assert!(visual.any_lit(now));
        assert!(!visual.any_playing(now));
        // A window that stopped painting stops counting here too.
        visual
            .panes
            .insert(2, lit_pane((800, 600), now - WINDOW_STALE * 2));
        assert!(!visual.any_playing(now));
    }
}
