//! Per-panel surface shaders: any panel can carry a WGSL fragment stage
//! that runs over its own body rect, layered under the app-wide post
//! shader. The config rides [`PanelChrome`](super::PanelChrome), so
//! persistence, duplication, and workspace bundles come free; the render
//! side is [`PanelSurface`], recorded by the [`Themed`](super::themed)
//! wrapper after the panel's body has painted.
//!
//! Three pieces live here because the upcoming Shader panel wants them
//! too: the slot targets a route list resolves into, the `// @slot n:`
//! label convention, and the eight `meta` floats every rox shader can
//! count on.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{App, Bounds, EntityId, Global, Pixels, UserShaderId, WeakEntity, Window, WindowId};
use serde::{Deserialize, Serialize};

use rox_viz::signal::{Route, SignalHub};

use crate::signal_ui::{self, RouteTargets};
use rox_services::player::Player;

use super::{AppState, PanelChrome};

/// How many signal slots a shader sees, the uniform block's width.
pub const SLOTS: usize = 16;

/// The builtins, shared by both shader surfaces: the Shader panel offers
/// them as presets, and the gate below trusts them by construction. One of
/// each kind on purpose - Plasma is a pure primitive, Trails reads its own
/// last frame and proves the region pass.
pub const PLASMA: &str = include_str!("shader/plasma.wgsl");
pub const TRAILS: &str = include_str!("shader/trails.wgsl");

pub const PRESETS: &[(&str, &str)] = &[("Plasma", PLASMA), ("Trails", TRAILS)];

/// How often a watched source file gets stat'd while its surface draws.
/// Twice a second: fast enough that a save in the editor lands before the
/// hand is back on the mouse, slow enough to be one syscall rather than one
/// a frame.
pub const RELOAD_EVERY: Duration = Duration::from_millis(500);

/// A panel's surface shader as it persists: the source text inline, the
/// file it was last loaded from, and the routes feeding its slots.
///
/// The source is stored inline on purpose. A workspace bundle carrying
/// only an absolute path would import as a dead shader on anyone else's
/// machine, so the path is a bookmark for the reload button and the
/// source is what actually runs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelShader {
    /// The switch. Off keeps the source and routes in place, unpainted.
    pub enabled: bool,
    /// The fragment stage: a `fs_user(uv)` definition, plus whatever it
    /// calls. Empty means nothing to run.
    pub source: String,
    /// Where the source was last read from, for the reload button. None
    /// once a bundle travels to a machine that never had the file.
    pub path: Option<PathBuf>,
    /// The signal routes filling the shader's slots.
    pub routes: Vec<Route>,
    /// Keep asking for frames with the hub silent. Off, a shader over a
    /// paused player freezes where it stands and the panel costs nothing.
    pub run_when_idle: bool,
}

impl Default for PanelShader {
    fn default() -> Self {
        PanelShader {
            enabled: true,
            source: String::new(),
            path: None,
            routes: Vec::new(),
            run_when_idle: false,
        }
    }
}

impl PanelShader {
    /// Whether there is anything to paint: switched on with source text.
    pub fn runnable(&self) -> bool {
        self.enabled && !self.source.trim().is_empty()
    }
}

/// A source's identity in the approved list: hex SHA-256 of the trimmed
/// text. Trimmed so an editor's trailing newline isn't a different program,
/// and hashed rather than stored so the list stays a few lines whatever the
/// shaders weigh.
pub fn fingerprint(source: &str) -> String {
    use sha2::{Digest as _, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(source.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Whether a source is one the app ships. Builtins are approved by
/// construction: they came with the binary, so a list entry would only be a
/// second copy of a decision already made by installing rox.
pub fn builtin(source: &str) -> bool {
    let source = source.trim();
    PRESETS.iter().any(|(_, preset)| preset.trim() == source)
}

/// Whether this source may run on this machine.
///
/// Shaders ride layout dumps and workspace bundles as inline WGSL, so
/// applying somebody else's look hands rox somebody else's code. Nothing
/// registers until its hash is in the machine-local approved list, which
/// only a direct action writes to: a file pick, a reload, a preset, or the
/// Approve button on the panel's settings page. An empty source reads as
/// approved because there is nothing to run.
pub fn approved(source: &str) -> bool {
    source.trim().is_empty()
        || builtin(source)
        || rox_core::settings::shader_approved(&fingerprint(source))
}

/// Record a source as approved, on this machine and on disk. Every path
/// where the user themselves put the source there calls this; nothing on
/// the apply or restore side ever does.
pub fn approve(source: &str) {
    if source.trim().is_empty() || builtin(source) {
        return;
    }
    rox_core::settings::approve_shader(&fingerprint(source));
}

/// The mtime watch behind hot reload, worn by both shader surfaces: the
/// Shader panel over its own config, and [`PanelSurface`] over a panel's
/// chrome. An external editor plus this is the authoring loop, so it never
/// prompts and never asks for a frame of its own - it rides the paint the
/// shader was already asking for.
#[derive(Default)]
pub struct SourceWatch {
    /// The file's size and mtime when it was last read.
    stamp: Option<(u64, i64)>,
    /// Whether a stamp has been taken for the source in hand. Unseeded, the
    /// first check reads the file whatever the stamp says, so an edit made
    /// while rox was closed lands on open rather than on the edit after it.
    seeded: bool,
    /// The last stat, so the check costs a syscall every
    /// [`RELOAD_EVERY`] rather than one a frame.
    checked: Option<Instant>,
}

impl SourceWatch {
    /// A watch for a source that was just read from `path`, so the next
    /// edit is what wakes it. A source with no file behind it gets an
    /// unseeded watch that never has anything to poll.
    pub fn seeded(path: Option<&Path>) -> SourceWatch {
        SourceWatch {
            stamp: path.and_then(rox_core::settings::file_stamp),
            seeded: path.is_some(),
            checked: Some(Instant::now()),
        }
    }

    /// The file's contents when it has moved since the last look, or None
    /// when it hasn't, when the throttle hasn't elapsed, or when the file
    /// has gone. A file that disappears leaves the running source alone -
    /// that is the whole reason the source is stored inline - and the watch
    /// stays armed for it coming back.
    pub fn poll(&mut self, path: &Path) -> Option<String> {
        let now = Instant::now();
        if self
            .checked
            .is_some_and(|last| now.duration_since(last) < RELOAD_EVERY)
        {
            return None;
        }
        self.checked = Some(now);
        let stamp = rox_core::settings::file_stamp(path)?;
        if self.seeded && self.stamp == Some(stamp) {
            return None;
        }
        self.seeded = true;
        self.stamp = Some(stamp);
        std::fs::read_to_string(path).ok()
    }
}

/// The target id a route uses to drive slot `n`.
pub fn slot_target(slot: usize) -> String {
    format!("slot{slot}")
}

/// The slot a target id drives, if it names one at all.
pub fn target_slot(id: &str) -> Option<usize> {
    let slot: usize = id.strip_prefix("slot")?.parse().ok()?;
    (slot < SLOTS).then_some(slot)
}

/// The slot names a shader declares, read off `// @slot n: name` comments
/// in its source. Anything the source doesn't name comes back None and
/// falls through to [`slot_label`]'s generic wording, so an unannotated
/// shader still binds.
pub fn slot_labels(source: &str) -> Vec<Option<String>> {
    let mut labels = vec![None; SLOTS];
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("//") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("@slot") else {
            continue;
        };
        let Some((index, name)) = rest.trim_start().split_once(':') else {
            continue;
        };
        let (Ok(index), name) = (index.trim().parse::<usize>(), name.trim()) else {
            continue;
        };
        if index < SLOTS && !name.is_empty() {
            labels[index] = Some(name.to_string());
        }
    }
    labels
}

/// A slot's display name: what the shader called it, or its number.
pub fn slot_label(labels: &[Option<String>], slot: usize) -> String {
    match labels.get(slot).and_then(|name| name.clone()) {
        Some(name) => name,
        None => format!("slot {slot}"),
    }
}

/// The sixteen slots a route list resolves into, the shader's side of
/// [`RouteTargets`]. Labels only matter to the picker; the paint path
/// builds these bare.
pub struct SlotTargets {
    pub slots: [f32; SLOTS],
    labels: Vec<Option<String>>,
}

impl Default for SlotTargets {
    fn default() -> Self {
        SlotTargets {
            slots: [0.0; SLOTS],
            labels: vec![None; SLOTS],
        }
    }
}

impl SlotTargets {
    /// Targets that report the shader's own slot names. Only a surface
    /// that lists targets needs these; the panel wrapper resolves routes
    /// without ever asking what a slot is called. The shader panel's
    /// Bindings page is the caller that does.
    pub fn labelled(source: &str) -> Self {
        SlotTargets {
            slots: [0.0; SLOTS],
            labels: slot_labels(source),
        }
    }
}

impl RouteTargets for SlotTargets {
    fn targets(&self) -> Vec<(String, String)> {
        (0..SLOTS)
            .map(|slot| (slot_target(slot), slot_label(&self.labels, slot)))
            .collect()
    }

    fn apply(&mut self, id: &str, value: f32) {
        if let Some(slot) = target_slot(id) {
            self.slots[slot] = value;
        }
    }
}

/// The workspace state a window's panels feed their shaders from. Panels
/// paint far from any `AppState`, so each window registers its hub and
/// player once and the wrapper looks them up by window, the same
/// window-keyed shape the art tint and the workspace registry use.
#[derive(Default)]
struct ShaderFeeds(HashMap<WindowId, Feed>);

impl Global for ShaderFeeds {}

struct Feed {
    hub: Arc<SignalHub>,
    player: WeakEntity<Player>,
}

/// Register a window's signal hub and player, so any panel painting in it
/// can resolve routes and meta. Called once as the window opens; a second
/// call for the same window replaces the entry. Windows that closed since
/// drop out here, so a stale hub isn't held alive forever.
///
/// Liveness is the window's, not the player's: a popped-out panel shares
/// its parent workspace's player, so closing the popout leaves that player
/// very much alive and its entry would sit here for the rest of the
/// session. The player check stays for the other direction, a window whose
/// workspace went away first.
pub fn note_window(window: &Window, state: &AppState, cx: &mut App) {
    let id = window.window_handle().window_id();
    let live: HashSet<WindowId> = cx.windows().iter().map(|h| h.window_id()).collect();
    let feeds = cx.default_global::<ShaderFeeds>();
    feeds
        .0
        .retain(|window, feed| live.contains(window) && feed.player.upgrade().is_some());
    feeds.0.insert(
        id,
        Feed {
            hub: state.signals.clone(),
            player: state.player.downgrade(),
        },
    );
}

/// The last compile message per panel, for its settings window's readout.
/// A panel whose shader compiles clean has no entry.
static ERRORS: LazyLock<RwLock<HashMap<EntityId, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// What a panel's shader last said, or None on a clean compile.
pub fn error(panel: EntityId) -> Option<String> {
    ERRORS.read().unwrap().get(&panel).cloned()
}

/// Store (or clear, with None) a panel's compile message. The paint path
/// writes what registration said; the settings window clears it when the
/// source moves on, and writes its own when a file won't read.
pub fn note_error(panel: EntityId, message: Option<String>) {
    let mut errors = ERRORS.write().unwrap();
    match message {
        Some(message) => {
            errors.insert(panel, message);
        }
        None => {
            errors.remove(&panel);
        }
    }
}

/// Sources that failed to compile, keyed by window and source hash. gpui
/// caches successful registrations by content, but a rejection re-runs
/// naga every call, and the wrapper registers from paint - so a broken
/// shader would re-validate on every unrelated repaint without this.
static FAILED: LazyLock<RwLock<HashMap<(u64, u64), String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn source_hash(source: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// What a panel's surface is running, per window it draws in. The wrapper
/// paints from an element with nothing but the panel's entity id to hand,
/// so the watch and the last good registration live out here rather than on
/// the panel. Keyed by window as well as panel because a popped-out panel
/// draws in two, and a `UserShaderId` belongs to the window that made it.
struct Live {
    /// The config source this entry was armed for. An edit in the settings
    /// window moves it, which re-arms the watch instead of letting the file
    /// pull the old text back over the edit.
    config: u64,
    watch: SourceWatch,
    /// The file's text, once a reload has moved past the config's copy.
    hot: Option<String>,
    /// The last registration that compiled clean, kept painting while a
    /// fresh edit is broken so an authoring loop doesn't strobe the panel
    /// off and on with every unfinished save.
    good: Option<UserShaderId>,
    /// Last time the entry was painted from, so entries for panels that
    /// closed don't hold their sources forever.
    touched: Instant,
}

static LIVE: LazyLock<RwLock<HashMap<(u64, EntityId), Live>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// How long an untouched entry sticks around before the next insert drops
/// it. Long enough that a panel in a background window keeps its state.
const LIVE_TTL: Duration = Duration::from_secs(300);

/// The source a panel's surface is actually running, when a hot reload has
/// moved it past the copy in the config. The settings window folds this back
/// into the config, so a layout saved after an external edit carries the
/// text that was on screen.
pub fn hot_source(panel: EntityId) -> Option<String> {
    LIVE.read()
        .unwrap()
        .iter()
        .find(|((_, id), live)| *id == panel && live.hot.is_some())
        .and_then(|(_, live)| live.hot.clone())
}

/// The render side of a panel's shader, built fresh each render from the
/// chrome and carried by the [`Themed`](super::themed) wrapper.
pub struct PanelSurface {
    source: String,
    /// The file the source was last read from, watched for edits.
    path: Option<PathBuf>,
    routes: Vec<Route>,
    run_when_idle: bool,
    /// The chrome margin, so the shader covers the panel's body rect and
    /// leaves the gutter the backdrop shows through alone.
    inset: Pixels,
}

impl PanelSurface {
    /// The surface a chrome asks for, or None when it carries no runnable
    /// shader - which includes one waiting on approval. An unapproved
    /// source builds no surface at all, so the panel renders exactly as it
    /// would with the shader switched off, and the Shader page is where the
    /// pending source and its Approve button live.
    pub fn build(chrome: &PanelChrome, margin: f32) -> Option<PanelSurface> {
        let shader = chrome.shader.as_ref().filter(|s| s.runnable())?;
        if !approved(&shader.source) {
            return None;
        }
        Some(PanelSurface {
            source: shader.source.clone(),
            path: shader.path.clone(),
            routes: shader.routes.clone(),
            run_when_idle: shader.run_when_idle,
            inset: gpui::px(margin.max(0.0)),
        })
    }

    /// Record the shader over the panel's body, after the body itself has
    /// painted. A source that won't compile keeps the last good one on
    /// screen and leaves its message for the panel's settings window;
    /// everything no-ops on a backend without a shader pipeline, which
    /// registration reports the same way.
    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let panel = window.current_view();
        let window_id = window.window_handle().window_id().as_u64();
        let (source, last_good) = self.current(window_id, panel);
        let key = (window_id, source_hash(&source));
        let failed = FAILED.read().unwrap().get(&key).cloned();
        let shader = match failed {
            Some(message) => {
                note_error(panel, Some(message));
                last_good
            }
            None => match window.register_user_shader(&source) {
                Ok(shader) => {
                    note_error(panel, None);
                    self.note_good(window_id, panel, shader);
                    Some(shader)
                }
                Err(message) => {
                    FAILED.write().unwrap().insert(key, message.clone());
                    note_error(panel, Some(message));
                    last_good
                }
            },
        };
        // Nothing has ever compiled here, so there is nothing to keep on
        // screen either. The message is on its way to the settings window.
        let Some(shader) = shader else {
            return;
        };
        let (signals, live) = self.signals(window, cx);
        let meta = meta_slots(window, cx);
        let bounds = body_rect(bounds, self.inset);
        // Caps decide the path: a shader that reads the screen under it or
        // its own last frame needs the region pass, and one that draws from
        // nothing but its uniforms is a plain in-scene quad. Getting this
        // backwards paints nothing at all, since each call skips what it
        // can't run.
        let screen = window
            .user_shader_caps(shader)
            .is_some_and(|caps| caps.samples_screen || caps.uses_prev);
        if screen {
            window.paint_screen_shader(bounds, shader, panel.as_u64(), signals, meta);
        } else {
            window.paint_user_shader(bounds, shader, signals, meta);
        }
        // Docked panels render cached: a clean frame replays the recorded
        // primitive with the values it was recorded with, so an animating
        // shader needs its panel dirtied every frame. `request_animation_frame`
        // notifies exactly this view, which is the cheap wake - a window
        // `refresh` would rebuild every view in the window uncached and
        // stall the whole frame loop.
        if live || self.run_when_idle {
            window.request_animation_frame();
        }
    }

    /// The source to run this frame and the last one that compiled, taking
    /// the hot reload with it: the watch stats the config's file every
    /// [`RELOAD_EVERY`], and a file that has moved becomes what runs until
    /// the settings window folds it back into the config.
    ///
    /// The reload only happens for a surface that is already painting, which
    /// means already approved. A pending source never gets here, so a bundle
    /// can't have rox read a path of its choosing and trust what comes back.
    fn current(&self, window: u64, panel: EntityId) -> (String, Option<UserShaderId>) {
        let config = source_hash(&self.source);
        let mut fresh = None;
        let (source, good) = {
            let mut live = LIVE.write().unwrap();
            let entry = match live.entry((window, panel)) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // A panel restored from a layout has a source snapshot
                    // and maybe a path, with no telling whether they still
                    // agree, so its watch starts unseeded and reads once.
                    entry.insert(Live {
                        config,
                        watch: SourceWatch::default(),
                        hot: None,
                        good: None,
                        touched: Instant::now(),
                    })
                }
            };
            if entry.config != config {
                // The settings window wrote a new source; it wins over
                // whatever the file said, and the watch re-arms from it.
                entry.config = config;
                entry.hot = None;
                entry.watch = SourceWatch::seeded(self.path.as_deref());
            }
            entry.touched = Instant::now();
            if let Some(path) = &self.path {
                if let Some(text) = entry.watch.poll(path) {
                    let running = entry.hot.as_deref().unwrap_or(&self.source);
                    if text.trim() != running.trim() {
                        // The user pointed rox at this file, so what comes
                        // out of it is theirs; approving here is what keeps
                        // the edit from tripping the gate on restart.
                        fresh = Some(text.clone());
                        entry.hot = Some(text);
                    }
                }
            }
            (
                entry.hot.clone().unwrap_or_else(|| self.source.clone()),
                entry.good,
            )
        };
        // Outside the lock: approving writes the settings file, and no other
        // panel's paint should wait on that.
        if let Some(text) = fresh {
            approve(&text);
        }
        (source, good)
    }

    /// Remember a clean registration as this surface's fallback, and drop
    /// the entries of panels that stopped drawing a while back.
    fn note_good(&self, window: u64, panel: EntityId, shader: UserShaderId) {
        let mut live = LIVE.write().unwrap();
        if let Some(entry) = live.get_mut(&(window, panel)) {
            entry.good = Some(shader);
        }
        if live.len() > 32 {
            let now = Instant::now();
            live.retain(|_, entry| now.duration_since(entry.touched) < LIVE_TTL);
        }
    }

    /// This frame's slot values, and whether the hub is moving. The tick
    /// happens here because a panel shader can be the only thing in the
    /// window watching the audio; it's deduped inside the hub, so several
    /// shaded panels cost one.
    fn signals(&self, window: &Window, cx: &App) -> ([f32; SLOTS], bool) {
        let mut targets = SlotTargets::default();
        let Some((hub, player)) = window_feed(window, cx) else {
            return (targets.slots, false);
        };
        {
            let player = player.read(cx);
            hub.tick(&player.feed(), player.playing_entry());
        }
        signal_ui::apply_routes(&self.routes, &hub, &mut targets);
        (targets.slots, hub.live())
    }
}

/// The panel's body rect: the wrapper's bounds pulled in by the chrome
/// margin, so the shader covers what the panel draws and not the gap
/// around it.
fn body_rect(bounds: Bounds<Pixels>, inset: Pixels) -> Bounds<Pixels> {
    let inset = inset
        .min(bounds.size.width / 2.0)
        .min(bounds.size.height / 2.0)
        .max(gpui::px(0.));
    Bounds {
        origin: bounds.origin + gpui::point(inset, inset),
        size: gpui::size(
            bounds.size.width - inset * 2.0,
            bounds.size.height - inset * 2.0,
        ),
    }
}

fn window_feed(window: &Window, cx: &App) -> Option<(Arc<SignalHub>, gpui::Entity<Player>)> {
    let feeds = cx.try_global::<ShaderFeeds>()?;
    let feed = feeds.0.get(&window.window_handle().window_id())?;
    Some((feed.hub.clone(), feed.player.upgrade()?))
}

/// The eight `meta` floats every rox shader can count on, the convention
/// the Shader panel shares: volume, where the track sits, whether audio is
/// moving, and how long the track runs. The last four are reserved and
/// read zero, so a shader written against them today keeps working when
/// they fill in.
pub fn meta_slots(window: &Window, cx: &App) -> [f32; 8] {
    let mut meta = [0.0f32; 8];
    let Some((_, player)) = window_feed(window, cx) else {
        return meta;
    };
    let player = player.read(cx);
    // The persisted volume runs to 200%; the slot is documented 0..1, so a
    // boosted level reads as full rather than pushing the slot past it.
    meta[0] = if player.muted() {
        0.0
    } else {
        player.volume().clamp(0.0, 1.0)
    };
    if let Some(now) = player.now_playing() {
        let duration = now.duration_secs.unwrap_or(0.0);
        if duration > 0.0 {
            meta[1] = (now.position_secs / duration).clamp(0.0, 1.0) as f32;
        }
        meta[3] = duration as f32;
    }
    meta[2] = if player.is_playing() { 1.0 } else { 0.0 };
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_viz::signal::Source;
    use rox_viz::AudioFeed;

    #[test]
    fn slot_targets_round_trip() {
        for slot in 0..SLOTS {
            assert_eq!(target_slot(&slot_target(slot)), Some(slot));
        }
        assert_eq!(target_slot("slot16"), None);
        assert_eq!(target_slot("bass"), None);
        assert_eq!(target_slot(""), None);
    }

    #[test]
    fn slot_labels_read_the_comment_convention() {
        let source = "// @slot 0: bass\n\
                      //@slot 3 : the  drums \n\
                      // @slot 99: out of range\n\
                      // @slot two: not a number\n\
                      // just a comment\n\
                      fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        let labels = slot_labels(source);
        assert_eq!(labels[0].as_deref(), Some("bass"));
        assert_eq!(labels[3].as_deref(), Some("the  drums"));
        assert_eq!(labels[1], None);
        assert_eq!(slot_label(&labels, 0), "bass");
        assert_eq!(slot_label(&labels, 7), "slot 7");
    }

    /// A hub carrying one band signal, run up to full off a tone in that
    /// band. The engine's attack takes a stretch of wall clock (the tick
    /// throttles), so this walks it there rather than faking a value.
    fn loud_hub() -> (SignalHub, u64) {
        let hub = SignalHub::new(Vec::new());
        let (id, _) = hub.add(
            Source::Band {
                lo: 800.0,
                hi: 2000.0,
            },
            0.0,
        );
        let feed = AudioFeed::new();
        // 1.17 kHz at 48 kHz, the midrange tone the engine's own tests use.
        let mut phase = 0.0f32;
        for _ in 0..60 {
            let mut samples = vec![0.0f32; 4096];
            for frame in samples.chunks_mut(2) {
                phase += std::f32::consts::TAU * 1170.0 / 48_000.0;
                frame[0] = phase.sin();
                frame[1] = frame[0];
            }
            feed.push(&samples);
            hub.tick(&feed, None);
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        (hub, id)
    }

    #[test]
    fn routes_resolve_into_slots() {
        let (hub, loud) = loud_hub();
        assert!(
            hub.value(loud).unwrap_or(0.0) > 0.5,
            "the band signal should be up before the routes are read"
        );

        let route = |signal, target: String, from, to, enabled| Route {
            enabled,
            signal,
            target,
            from,
            to,
        };
        let routes = vec![
            route(loud, slot_target(2), 0.0, 1.0, true),
            // Half the span, so the same signal lands at half strength.
            route(loud, slot_target(5), 0.0, 0.5, true),
            // Off, so slot 7 stays at rest.
            route(loud, slot_target(7), 0.0, 1.0, false),
            // A signal the pool never carried contributes nothing.
            route(999, slot_target(9), 0.0, 1.0, true),
            // A target nothing answers to is skipped, not a panic.
            route(loud, "nowhere".to_string(), 0.0, 1.0, true),
            // Out of range reads as no slot at all.
            route(loud, slot_target(SLOTS), 0.0, 1.0, true),
        ];
        let mut targets = SlotTargets::default();
        signal_ui::apply_routes(&routes, &hub, &mut targets);

        let full = targets.slots[2];
        assert!(full > 0.5, "slot 2 should carry the signal, got {full}");
        assert!(
            (targets.slots[5] - full * 0.5).abs() < 0.05,
            "slot 5 should sit at half the span"
        );
        assert_eq!(targets.slots[7], 0.0);
        assert_eq!(targets.slots[9], 0.0);
        assert_eq!(targets.slots[0], 0.0);
    }

    #[test]
    fn targets_list_every_slot_by_name() {
        let targets = SlotTargets::labelled("// @slot 1: mids\n");
        let listed = targets.targets();
        assert_eq!(listed.len(), SLOTS);
        assert_eq!(listed[1], ("slot1".to_string(), "mids".to_string()));
        assert_eq!(listed[4], ("slot4".to_string(), "slot 4".to_string()));
    }

    /// A source no list will ever carry, unique per call so two tests
    /// approving at once can't see each other's.
    fn novel_source(tag: &str) -> String {
        format!(
            "// {tag} {:?}\nfn fs_user(uv: vec2<f32>) -> vec4<f32> {{ return vec4<f32>(uv, 0.0, 1.0); }}",
            std::time::SystemTime::now()
        )
    }

    #[test]
    fn a_source_that_arrives_serialized_waits() {
        let source = novel_source("arrived");
        assert!(
            !approved(&source),
            "a source nobody has agreed to must not run"
        );
        // What the Approve button does, minus the settings write (which
        // would land in the machine's real session file).
        let print = fingerprint(&source);
        assert!(rox_core::settings::note_approved(&print));
        assert!(approved(&source), "an approved hash runs");
        // The same program with a different name in it is a different
        // program, and doesn't ride the first one's approval.
        assert!(!approved(&novel_source("arrived twice")));
        rox_core::settings::forget_approved(&print);
        assert!(!approved(&source), "and the gate closes again");
    }

    #[test]
    fn the_builtins_need_no_list() {
        for (label, preset) in PRESETS {
            assert!(builtin(preset), "{label} is one of ours");
            assert!(approved(preset), "{label} ships with the binary");
            assert!(
                !rox_core::settings::shader_approved(&fingerprint(preset)),
                "{label} shouldn't need a list entry to pass the gate"
            );
        }
        // Approving a builtin is a no-op rather than a list entry, so the
        // file doesn't fill up with hashes of what shipped.
        approve(PLASMA);
        assert!(!rox_core::settings::shader_approved(&fingerprint(PLASMA)));
        // Nothing to run is nothing to gate.
        assert!(approved(""));
        assert!(approved("   \n "));
    }

    #[test]
    fn fingerprints_ignore_the_edges_and_nothing_else() {
        let source = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        assert_eq!(fingerprint(source), fingerprint(&format!("\n{source}\n\n")));
        assert_ne!(
            fingerprint(source),
            fingerprint(&source.replace("1.0", "0.0")),
            "a changed constant is a changed shader"
        );
        // Hex of a SHA-256, so the list stays readable and fixed width.
        let print = fingerprint(source);
        assert_eq!(print.len(), 64);
        assert!(print.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_builtin_survives_a_round_trip_through_a_layout() {
        // Presets ride a dump as inline source like anything else, and
        // serde's string round trip is where a trailing newline would go
        // missing. The gate has to still know it as ours on the way back.
        let dumped = serde_json::to_string(&PLASMA.to_string()).expect("dump");
        let read: String = serde_json::from_str(&dumped).expect("read");
        assert!(approved(&read));
    }

    /// A file to watch, in a directory of this test's own so a parallel
    /// test run can't stat somebody else's writes.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-shader-watch-{name}"));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("shader.wgsl")
    }

    #[test]
    fn an_unseeded_watch_reads_once_then_waits_for_the_file_to_move() {
        let path = scratch("unseeded");
        std::fs::write(&path, "one").expect("write");
        let mut watch = SourceWatch::default();
        // Unseeded, so an edit made while rox was closed lands on open
        // rather than on the edit after it.
        assert_eq!(watch.poll(&path).as_deref(), Some("one"));
        // Throttled: the next look inside the window costs no syscall and
        // reports nothing.
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        assert_eq!(watch.poll(&path), None, "nothing moved");
        // The stamp is size and mtime, and mtime only resolves to the
        // second, so the change here is a length.
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_seeded_watch_waits_for_the_next_edit() {
        let path = scratch("seeded");
        std::fs::write(&path, "one").expect("write");
        // What a file pick leaves behind: the source was just read from
        // here, so the file as it stands is not news.
        let mut watch = SourceWatch::seeded(Some(path.as_path()));
        watch.checked = None;
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        // A file that goes missing leaves the running source alone and the
        // watch armed for it coming back.
        watch.checked = None;
        std::fs::remove_file(&path).ok();
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        std::fs::write(&path, "back again, longer").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("back again, longer"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_watch_with_no_file_has_nothing_to_seed() {
        let watch = SourceWatch::seeded(None);
        assert!(!watch.seeded);
        assert!(watch.stamp.is_none());
    }

    #[test]
    fn body_rect_pulls_in_by_the_margin() {
        let bounds = Bounds {
            origin: gpui::point(gpui::px(10.), gpui::px(20.)),
            size: gpui::size(gpui::px(100.), gpui::px(50.)),
        };
        let inner = body_rect(bounds, gpui::px(5.));
        assert_eq!(inner.origin.x, gpui::px(15.));
        assert_eq!(inner.origin.y, gpui::px(25.));
        assert_eq!(inner.size.width, gpui::px(90.));
        assert_eq!(inner.size.height, gpui::px(40.));
        // A margin wider than the panel can't invert the rect.
        let squeezed = body_rect(bounds, gpui::px(400.));
        assert!(squeezed.size.width >= gpui::px(0.));
        assert!(squeezed.size.height >= gpui::px(0.));
    }
}
