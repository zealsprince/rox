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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use gpui::{App, Bounds, EntityId, Global, Pixels, WeakEntity, Window, WindowId};
use serde::{Deserialize, Serialize};

use rox_viz::signal::{Route, SignalHub};

use crate::player::Player;
use crate::signal_ui::{self, RouteTargets};

use super::{AppState, PanelChrome};

/// How many signal slots a shader sees, the uniform block's width.
pub const SLOTS: usize = 16;

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
pub fn note_window(window: &Window, state: &AppState, cx: &mut App) {
    let id = window.window_handle().window_id();
    let feeds = cx.default_global::<ShaderFeeds>();
    feeds.0.retain(|_, feed| feed.player.upgrade().is_some());
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

/// The render side of a panel's shader, built fresh each render from the
/// chrome and carried by the [`Themed`](super::themed) wrapper.
pub struct PanelSurface {
    source: String,
    routes: Vec<Route>,
    run_when_idle: bool,
    /// The chrome margin, so the shader covers the panel's body rect and
    /// leaves the gutter the backdrop shows through alone.
    inset: Pixels,
}

impl PanelSurface {
    /// The surface a chrome asks for, or None when it carries no runnable
    /// shader. `margin` is the resolved frame margin, the gutter the body
    /// sits inside.
    pub fn build(chrome: &PanelChrome, margin: f32) -> Option<PanelSurface> {
        let shader = chrome.shader.as_ref().filter(|s| s.runnable())?;
        Some(PanelSurface {
            source: shader.source.clone(),
            routes: shader.routes.clone(),
            run_when_idle: shader.run_when_idle,
            inset: gpui::px(margin.max(0.0)),
        })
    }

    /// Record the shader over the panel's body, after the body itself has
    /// painted. A source that won't compile paints nothing and leaves its
    /// message for the panel's settings window; everything no-ops on a
    /// backend without a shader pipeline, which registration reports the
    /// same way.
    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let panel = window.current_view();
        let key = (
            window.window_handle().window_id().as_u64(),
            source_hash(&self.source),
        );
        if let Some(message) = FAILED.read().unwrap().get(&key).cloned() {
            note_error(panel, Some(message));
            return;
        }
        let shader = match window.register_user_shader(&self.source) {
            Ok(shader) => {
                note_error(panel, None);
                shader
            }
            Err(message) => {
                FAILED.write().unwrap().insert(key, message.clone());
                note_error(panel, Some(message));
                return;
            }
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
        let (id, _) = hub.add(Source::Band { lo: 800.0, hi: 2000.0 }, 0.0);
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
