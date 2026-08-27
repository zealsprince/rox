//! The main window: an in-window menubar over the dock area. gpui only
//! surfaces `set_menus` in the macOS system bar, so the bar is drawn
//! in-window to behave the same on every platform. The dock, tabs, splits,
//! and resize come from gpui-component per ADR 7; duplicate and pop-out
//! belong to the panels themselves. Playback UI is the transport panels in
//! the bottom dock; the PCM tap the audio views read is drained by the
//! player's own pump task, so nothing here has to keep rendering for
//! playback's sake.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use gpui::{
    actions, canvas, deferred, div, overlay_phase, prelude::*, px, svg, AnyElement,
    AnyWindowHandle, App, Axis, Bounds, Context, DismissEvent, Div, Entity, ExternalPaths,
    FocusHandle, Focusable as _, FontFeatures, Global, KeyDownEvent, Modifiers,
    ModifiersChangedEvent, MouseButton, PathPromptOptions, Pixels, SharedString, Subscription,
    Task, WeakEntity, Window, WindowBounds,
};
use rox_dock::{
    register_panel, DockArea, DockAreaState, DockEvent, DockItem, Panel as _, PanelInfo, PanelView,
    StackPanel, TabPanel, ToggleZoom,
};
use rox_library::cue::TrackKey;

use gpui::rgba;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::PopupMenu;
use gpui_component::Icon;

use crate::composite;
use crate::integrations::media_controls::MediaSession;
use crate::integrations::tray;
use crate::panel_catalog::{self as catalog, PanelDef, PanelPlacement, PanelSection};
use crate::panel_presets;
use crate::panels::drawer::DrawerPanel;
use crate::panels::group::GroupPanel;
use crate::panels::menu::{MenuConfig, MenuPanel};
use crate::panels::mini::{MiniToggleConfig, MiniTogglePanel};
use crate::panels::overlay::OverlayPanel;
use crate::panels::queue_widget::QueueWidgetPanel;
use crate::panels::slide::SlidePanel;
use crate::panels::window_controls::{WindowControlsConfig, WindowControlsPanel};
use crate::quick_play::QuickPlay;
use rox_core::settings::{
    self, LastTrack, LayoutEdit, LayoutSize, NamedLayout, PostShaderConfig, QueueState,
    QueuedTrack, Settings, WindowState,
};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, TabHosts};
use rox_panel_api::query::shared_query::SharedQuery;
use rox_panel_api::track_ui::track_drag::PlayDrag;
use rox_panel_kit::ui::{chord, kbd_line, Seg};
use rox_panels::art::{ArtConfig, ArtPanel};
use rox_panels::artist_grid::{ArtistGridConfig, ArtistGridPanel};
use rox_panels::biography::BiographyPanel;
use rox_panels::cover::CoverArtPanel;
use rox_panels::drag_anchor::DragAnchorPanel;
use rox_panels::eq_widget::EqWidgetPanel;
use rox_panels::filter::{FilterConfig, FilterPanel};
use rox_panels::folder_tree::FolderTreePanel;
use rox_panels::genre_grid::{GenreGridConfig, GenreGridPanel};
use rox_panels::grid::{GridConfig, GridPanel};
use rox_panels::history::HistoryPanel;
use rox_panels::library::{LibraryConfig, LibraryPanel};
use rox_panels::lyrics::LyricsPanel;
use rox_panels::metadata::MetadataPanel;
use rox_panels::oscilloscope::OscilloscopePanel;
use rox_panels::output::OutputPanel;
use rox_panels::particles::ParticlesPanel;
use rox_panels::playlists::PlaylistsPanel;
use rox_panels::queue::QueuePanel;
use rox_panels::search::{SearchConfig, SearchPanel};
use rox_panels::shader::ShaderPanel;
use rox_panels::spacer::SpacerPanel;
use rox_panels::spectrogram::SpectrogramPanel;
use rox_panels::spectrum::SpectrumPanel;
use rox_panels::stats_widget::StatsWidgetPanel;
use rox_panels::status::StatusPanel;
use rox_panels::theme_toggle::ThemeTogglePanel;
use rox_panels::transport::{SeekStripPanel, TrackInfoPanel, TransportPanel, VolumePanel};
use rox_panels::vu::VuPanel;
use rox_panels::waveform::WaveformPanel;
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::discord_presence::DiscordPresence;
use rox_services::history::{History, HistoryEvent};
use rox_services::lastfm::Scrobbler;
use rox_services::player::Player;
use rox_services::portraits::Portraits;
use rox_services::selection::Selection;
use rox_services::thumbs::Thumbs;
use rox_viz::signal::{Route, SignalHub};

mod menubar;
pub(crate) mod native_menu;

const MENU_BAR_H: f32 = 30.0;

/// How long an Alt press may last and still read as a tap rather than a
/// hold. A hold is the plain reveal; two taps pin the bar up.
const ALT_TAP_MAX: Duration = Duration::from_millis(400);

/// How far apart two Alt taps may be and still count as a double-tap.
const ALT_DOUBLE_TAP: Duration = Duration::from_millis(500);

// The registry of open workspace windows is in rox-panel-api now, so the
// app-level lookups panels make can resolve without the Workspace type. It
// holds the entity type-erased; the typed wrappers below downcast it back.
// front_workspace returns the handle and the shared state, which is all its
// callers (the tray, the taskbar, the tasks/EQ/signals/console windows, the
// single-instance guard) need.
use rox_panel_api::windows::OpenWorkspace;
use rox_panel_api::windows::{note_activated, WorkspaceWindows};

/// The workspace behind a registry entry, or None once its entity has
/// gone. The registry stores it type-erased so nothing below the binary
/// has to know this type.
fn typed_workspace(workspace: &gpui::AnyWeakEntity) -> Option<Entity<Workspace>> {
    workspace.upgrade()?.downcast::<Workspace>().ok()
}

/// Renegotiate every workspace window's decorations to the live flag.
/// Only the main windows follow it; child windows (settings, popouts,
/// editors) keep the OS chrome. Called from the Window menu toggle and
/// the settings window's Appearance page.
pub(crate) fn apply_decorations(cx: &mut App) {
    // Deferred out of the caller's update: the menu toggle runs inside the
    // very window this renegotiates, and a window can't be updated while it's
    // already on the update stack. The re-entrant update errs and the window
    // silently keeps its old chrome until restart.
    cx.defer(|cx| {
        let mode = settings::window_decorations();
        let open: Vec<AnyWindowHandle> = cx
            .default_global::<WorkspaceWindows>()
            .open
            .iter()
            .map(|w| w.handle)
            .collect();
        for handle in open {
            handle
                .update(cx, |_, window, _| window.request_decorations(mode))
                .ok();
        }
        // Every window repaints, not just the renegotiated ones: the settings
        // window's Appearance toggle reads the flag live and would show stale
        // otherwise.
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// Push the live resize-border flag at every workspace window, the
/// decorations apply's twin. A no-op off Windows, where gpui leaves the
/// call unimplemented. Deferred for the same reason: the Window menu runs
/// inside one of the windows this loops over.
pub(crate) fn apply_resize_border(cx: &mut App) {
    cx.defer(|cx| {
        let on = settings::resize_border();
        let open: Vec<AnyWindowHandle> = cx
            .default_global::<WorkspaceWindows>()
            .open
            .iter()
            .map(|w| w.handle)
            .collect();
        for handle in open {
            handle
                .update(cx, |_, window, _| window.set_resize_border(on))
                .ok();
        }
    });
}

/// What the last post shader compile said, for the Shader settings page's
/// readout: None is a clean compile (or nothing installed). Shared across
/// windows because they all run the same file; the last one to compile
/// wins, which for one file is the same message.
static POST_SHADER_ERROR: RwLock<Option<String>> = RwLock::new(None);

/// The last post shader compile error, read live by the settings window.
pub(crate) fn post_shader_error() -> Option<String> {
    POST_SHADER_ERROR.read().unwrap().clone()
}

/// Put a line in that readout from outside the compile path, which is how
/// the settings page's own failures (an eject that won't write) end up in
/// the same place a broken shader's message does. The next apply clears
/// or overwrites it the way it always did.
pub(crate) fn note_post_shader_error(message: String) {
    *POST_SHADER_ERROR.write().unwrap() = Some(message);
}

/// The shader switch as it currently stands, a live copy of the settings
/// file's. A static like `hide_menubar`'s, because the Appearance toggle, the
/// menu row, and the hotkey all flip it and all have to show one state.
static POST_SHADER_ON: AtomicBool = AtomicBool::new(false);

pub(crate) fn post_shader_on() -> bool {
    POST_SHADER_ON.load(Ordering::Relaxed)
}

/// The screen shader's config as the last apply read it off the file, with
/// a counter that moves every time it does. The settings window's Shader
/// page keeps a copy of the config so it isn't reading five shards per
/// render, and a workspace apply swaps the whole thing from outside that
/// window, which left the picker naming the old look's shader. Watching the
/// counter is an atomic load per render; the config only gets cloned on the
/// frames where it actually moved.
static POST_SHADER_GEN: AtomicU64 = AtomicU64::new(0);
static POST_SHADER_APPLIED: RwLock<Option<PostShaderConfig>> = RwLock::new(None);

pub(crate) fn post_shader_gen() -> u64 {
    POST_SHADER_GEN.load(Ordering::Relaxed)
}

/// The config behind that counter, for a copy that has fallen behind it.
pub(crate) fn post_shader_applied() -> Option<PostShaderConfig> {
    POST_SHADER_APPLIED.read().unwrap().clone()
}

/// The screen shader's routes as the frame loop sees them. Live rather
/// than read from settings per frame for the obvious reason, but also so
/// editing a route never touches the file the shader is compiled from: a
/// settings reapply re-reads and recompiles the WGSL, which is not what a
/// slider drag should cost.
static POST_SHADER_ROUTES: RwLock<Vec<Route>> = RwLock::new(Vec::new());

/// Point the screen shader at a new route list. The settings window calls
/// this as it edits; the per-window apply seeds it from the file.
pub(crate) fn set_post_shader_routes(routes: Vec<Route>) {
    *POST_SHADER_ROUTES.write().unwrap() = routes;
}

/// The hand-set slot values next to the routes above, live for the same
/// reason: a slider drag on the Shader page has to show up on the next
/// frame without buying a settings reload.
static POST_SHADER_MANUAL: RwLock<Vec<(u8, f32)>> = RwLock::new(Vec::new());

/// Point the screen shader at a new hand-set list. Same callers as the
/// routes setter: the settings window as it edits, the apply from the file.
pub(crate) fn set_post_shader_manual(manual: Vec<(u8, f32)>) {
    *POST_SHADER_MANUAL.write().unwrap() = manual;
}

/// The slot names the screen shader's source declares, from its `// @slot
/// n: name` comments. Read where the file is, published here so the
/// settings window's route editor can name slots without opening the file
/// on every render.
static POST_SHADER_LABELS: RwLock<Vec<Option<String>>> = RwLock::new(Vec::new());

pub(crate) fn post_shader_slot_labels() -> Vec<Option<String>> {
    POST_SHADER_LABELS.read().unwrap().clone()
}

/// Whether what's installed says it leaves the window usable, published
/// beside the labels and for the same reason: the settings page has to say
/// what the shader does to the UI without opening a file per render, and in
/// file mode the driver's read is the only place the source is ever in
/// hand. Under search that page rebuilds on every keystroke, so a read here
/// would be a syscall per character typed anywhere in settings.
///
/// Three states, because "nothing is installed" and "a scene is installed"
/// need opposite words on screen: 0 nothing, 1 a scene, 2 an overlay.
static POST_SHADER_COVERAGE: AtomicU8 = AtomicU8::new(0);

/// What the installed screen shader does to the window: None with nothing
/// running, `Some(true)` for a shader that leaves the app usable under it.
pub(crate) fn post_shader_overlay() -> Option<bool> {
    match POST_SHADER_COVERAGE.load(Ordering::Relaxed) {
        0 => None,
        code => Some(code == 2),
    }
}

/// Flip the screen shader everywhere: the menu row and the hotkey. This is
/// the escape hatch for a shader that makes windows unusable, so it binds
/// unscoped, applies immediately, and never prompts in either direction.
pub(crate) fn toggle_post_shader(cx: &mut App) {
    let on = !Settings::load().post_shader.enabled;
    POST_SHADER_ON.store(on, Ordering::Relaxed);
    Settings::update(move |s| s.post_shader.enabled = on);
    apply_post_shader(cx);
}

/// The child windows currently running the shader under the all-windows
/// option, and the program they run: the composed source plus where its
/// images come from, since a child registers the same program the
/// workspace windows do. Workspace windows never appear here; they keep
/// their own [`PostShaderDriver`]s.
#[derive(Default)]
struct ShadedChildren {
    program: Option<(String, panel::shader::ProgramCtx)>,
    windows: Vec<AnyWindowHandle>,
}

impl Global for ShadedChildren {}

/// The shader confirm dialog's window while one is up. The sweeps skip it
/// unconditionally: it's the way back from a bad shader, so it can never
/// be shaded itself.
#[derive(Default)]
struct PostShaderConfirmWindow(Option<AnyWindowHandle>);

impl Global for PostShaderConfirmWindow {}

/// Register (or clear) the confirm dialog's window before any sweep can
/// reach it. Called by the dialog on open and release.
pub(crate) fn note_confirm_window(handle: Option<AnyWindowHandle>, cx: &mut App) {
    cx.default_global::<PostShaderConfirmWindow>().0 = handle;
}

/// The message for a screen shader that arrived inside a look and hasn't
/// been approved yet. Shown in the settings page's readout beside the compile
/// errors, since from where the user sits it's the same question: why is
/// nothing painting?
const UNAPPROVED_POST_SHADER: &str =
    "this shader arrived with a workspace and hasn't been approved on this machine";

/// The WGSL the screen shader actually runs and where it came from,
/// resolved the way a panel surface resolves its own.
///
/// A pool name wins outright: a hit runs the pool's copy, and a miss runs
/// nothing rather than falling through to whatever inline text happens to
/// be beside it. Then the inline source, which is how a shader
/// travels inside a bundle. Then the file the config points at, the way it
/// worked before either of the other two existed.
///
/// The first two go through the approval gate, because a bundle apply is
/// exactly the "somebody else's code" path the gate exists for. The file
/// read doesn't: picking a file is the agreement, and the pick already
/// recorded it.
///
/// The origin comes back with it because a program's images resolve from
/// it: the pool entry's stored bytes, or files beside the source. It's
/// decided here rather than by the drivers so there's one reading of where
/// a shader came from instead of one per surface.
///
/// `Ok(None)` is nothing to run, `Err` a line for the settings page's
/// readout.
pub(crate) fn post_shader_program(
    config: &PostShaderConfig,
) -> Result<Option<(String, panel::shader::ProgramCtx)>, String> {
    use panel::shader::ProgramCtx;

    // Empty text is nothing to run, whichever way in it arrived, and the
    // gate reads it as approved for the same reason.
    let gated = |source: String| match panel::shader::approved(&source) {
        true => Ok((!source.trim().is_empty()).then_some(source)),
        false => Err(UNAPPROVED_POST_SHADER.to_string()),
    };
    if let Some(name) = config.name.as_deref() {
        return match settings::shader_pool_get(name) {
            Some(entry) => Ok(gated(entry.source)?.map(|s| (s, ProgramCtx::named(name)))),
            None => Ok(None),
        };
    }
    if !config.source.trim().is_empty() {
        // Detached: an inline source arrived inside a layout, so there's
        // nothing on this machine holding the images it might declare.
        return Ok(gated(config.source.clone())?.map(|s| (s, ProgramCtx::detached())));
    }
    let Some(path) = config.path.as_ref() else {
        return Ok(None);
    };
    std::fs::read_to_string(path)
        .map(|source| Some((source, ProgramCtx::file(path))))
        .map_err(|e| format!("reading {}: {e}", path.display()))
}

/// Just the text, for the callers comparing one config's shader against
/// another's rather than compiling it.
pub(crate) fn post_shader_source(config: &PostShaderConfig) -> Result<Option<String>, String> {
    Ok(post_shader_program(config)?.map(|(source, _)| source))
}

/// The backdrop shader's last compile message, for the settings page's
/// readout: the surface notes errors against the workspace view that
/// painted it, which a settings window has no way to name, so the paint
/// publishes here as well. None is a clean compile or nothing running.
static BACKDROP_SHADER_ERROR: LazyLock<RwLock<Option<String>>> =
    LazyLock::new(|| RwLock::new(None));

/// What the backdrop shader last said, for the Backdrop section's banner.
pub(crate) fn backdrop_shader_error() -> Option<String> {
    BACKDROP_SHADER_ERROR.read().unwrap().clone()
}

/// Put a message on that readout from outside the paint, for the page's
/// own failures: a file that won't read, an eject that can't be written.
pub(crate) fn note_backdrop_shader_error(message: String) {
    *BACKDROP_SHADER_ERROR.write().unwrap() = Some(message);
}

/// Repaint every window, for edits that go into state the backdrop layers
/// read at render time: the Backdrop section writes its config through
/// here so the shader follows the knobs live, child windows included.
pub(crate) fn refresh_backdrop(cx: &mut App) {
    for handle in cx.windows() {
        handle.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// Hand the backdrop layer its shade, once at startup. The layer is in
/// rox-services, under the shader machinery, so it calls back up through
/// this hook; which windows get the shade is decided here, since the
/// service can't tell a workspace from a settings window.
pub(crate) fn install_backdrop_shade() {
    rox_services::backdrop::set_shade(|window, cx| {
        let config = settings::backdrop_shader().filter(|config| config.enabled)?;
        if !config.all_windows && !workspace_window(window, cx) {
            return None;
        }
        backdrop_shader_layer()
    });
    // The layer's gate goes on in the same install: whether a child window
    // paints the cover backdrop at all is the Transparency section's All
    // Windows switch, and the workspaces always do.
    rox_services::backdrop::set_gate(|window, cx| {
        rox_design::palette::backdrop_all_windows() || workspace_window(window, cx)
    });
}

/// Whether this window is one of the open workspaces, which always get
/// the backdrop shade; everything else is a child gated on All Windows.
fn workspace_window(window: &Window, cx: &App) -> bool {
    let id = window.window_handle().window_id();
    cx.try_global::<WorkspaceWindows>()
        .is_some_and(|windows| windows.open.iter().any(|w| w.handle.window_id() == id))
}

/// The render side of the look's backdrop shader: the config off the cache,
/// built as a panel surface so registration, approval, routes, and frame
/// pacing all come free. Built per render like any panel's; None is a bare
/// backdrop, a disabled config, or a source the machine hasn't approved.
fn backdrop_surface() -> Option<panel::shader::PanelSurface> {
    let config = settings::backdrop_shader()?;
    if !config.enabled {
        return None;
    }
    let chrome = panel::PanelChrome {
        shader: Some(panel::PanelShader {
            enabled: config.enabled,
            source: config.source,
            name: config.name,
            path: config.path,
            routes: config.routes,
            manual: config.manual,
            run_when_idle: config.run_when_idle,
        }),
        ..Default::default()
    };
    panel::shader::PanelSurface::build(&chrome, rox_design::palette::Sides::default())
}

/// The element that paints it: a window-filling canvas slotted between the
/// backdrop layer and everything else, so the screen the pass reads is the
/// art wash alone and every panel drawn after it goes over the result
/// untouched.
fn backdrop_shader_layer() -> Option<AnyElement> {
    let surface = backdrop_surface()?;
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        surface.paint(bounds, window, cx);
                        // The surface files its compile message under this
                        // window's view; copy it where the settings page
                        // can read it without knowing the view.
                        *BACKDROP_SHADER_ERROR.write().unwrap() =
                            panel::shader::error(window.current_view());
                    },
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

/// The file behind the screen shader, which is the only source hot reload
/// can watch. Set only when [`post_shader_source`] reads the file: a pool
/// entry or an inline source changes through an apply, never behind rox's
/// back, so there's nothing to stat for either of those.
fn post_shader_watch(config: &PostShaderConfig) -> Option<PathBuf> {
    if config.name.is_some() || !config.source.trim().is_empty() {
        return None;
    }
    config.path.clone()
}

/// Reapply the configured post shader everywhere. The Shader settings page's
/// controls, the toggle action, the confirm dialog's revert, and the hot
/// reload all come through here; each workspace window re-reads the file and
/// compiles it fresh, then the child windows follow when the all-windows
/// option is on. Deferred like the decorations apply, so an in-window
/// trigger can't re-enter its own update.
pub(crate) fn apply_post_shader(cx: &mut App) {
    cx.defer(|cx| {
        let config = Settings::load().post_shader;
        POST_SHADER_ON.store(config.enabled, Ordering::Relaxed);
        // Publish what the file said before any window compiles it, so a
        // settings window open over this apply can catch its copies up.
        *POST_SHADER_APPLIED.write().unwrap() = Some(config.clone());
        POST_SHADER_GEN.fetch_add(1, Ordering::Relaxed);
        let open: Vec<(AnyWindowHandle, Entity<Workspace>)> = cx
            .default_global::<WorkspaceWindows>()
            .open
            .iter()
            .filter_map(|w| Some((w.handle, typed_workspace(&w.workspace)?)))
            .collect();
        for (handle, workspace) in open {
            handle
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |workspace, cx| workspace.apply_post_shader(window, cx));
                })
                .ok();
        }
        // The child pass: cache the program and shade every eligible window,
        // or strip the ones shaded before. Resolved through the same order
        // the workspace windows use, so a child never runs something the
        // workspace isn't running. Errors fall through as None here; the
        // workspace pass above already surfaced them.
        let program = (config.enabled && config.all_windows)
            .then(|| post_shader_program(&config).ok().flatten())
            .flatten();
        let previous = std::mem::take(&mut cx.default_global::<ShadedChildren>().windows);
        let shading = program.is_some();
        cx.default_global::<ShadedChildren>().program = program;
        if shading {
            sweep_shaded_children(cx);
        } else {
            for handle in previous {
                handle
                    .update(cx, |_, window, _| {
                        window.set_post_shader(None);
                    })
                    .ok();
            }
        }
        // Repaint everything so the settings window's error readout and the
        // shaded windows come up in the same frame.
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// Shade every eligible window that isn't running the source yet: child
/// windows opened after the apply join through the workspace's periodic
/// sweep. Workspace windows keep their own drivers, and the confirm
/// dialog is always skipped.
fn sweep_shaded_children(cx: &mut App) {
    let Some((source, ctx)) = cx.default_global::<ShadedChildren>().program.clone() else {
        return;
    };
    let workspaces: Vec<AnyWindowHandle> = cx
        .default_global::<WorkspaceWindows>()
        .open
        .iter()
        .map(|w| w.handle)
        .collect();
    let confirm = cx.default_global::<PostShaderConfirmWindow>().0;
    let shaded = cx.default_global::<ShadedChildren>().windows.clone();
    // A child has no player to fill a `@cover` binding, so it borrows the
    // primary workspace's art, off the same window whose frame loop pushes
    // the children their signals.
    let cover_from = panel::shader::uses_cover(&source)
        .then(|| {
            cx.default_global::<WorkspaceWindows>()
                .open
                .iter()
                .min_by_key(|w| w.opened)
                .map(|w| w.handle.window_id().as_u64())
        })
        .flatten();
    for handle in cx.windows() {
        if workspaces.contains(&handle) || Some(handle) == confirm || shaded.contains(&handle) {
            continue;
        }
        let installed = handle
            .update(cx, |_, window, _| {
                if let Some(primary) = cover_from {
                    panel::shader::adopt_cover(
                        primary,
                        window.window_handle().window_id().as_u64(),
                    );
                }
                // Whole program, so a child runs the same chain and the
                // same images the workspace windows do. A failure here is
                // silent: the workspace pass compiled the same text and
                // already put the message in the readout.
                match panel::shader::register_program(window, &source, &ctx) {
                    Ok(id) => {
                        window.set_post_shader(Some(id));
                        true
                    }
                    Err(_) => false,
                }
            })
            .unwrap_or(false);
        if installed {
            cx.default_global::<ShadedChildren>().windows.push(handle);
        }
    }
}

/// Push this frame's signal values into every shaded child window and keep
/// their frames coming. Child windows have no driver of their own; the
/// primary workspace's frame loop calls this through a defer, since a
/// window can't update its siblings mid-render. The wake is a notify on
/// the child's root view, the same wake `request_animation_frame` uses:
/// it schedules a draw where everything but the root reuses its prepaint
/// cache. A `refresh` here instead sets the window-wide refreshing flag,
/// which rebuilds every view in the child uncached, and at frame cadence
/// across a few open windows that saturated the main thread and stalled
/// the shader clock everywhere. Each push notifies again, so the cadence
/// comes from the workspace loop.
fn push_child_signals(signals: [f32; 16], meta: [f32; 8], cx: &mut App) {
    let shaded = cx.default_global::<ShadedChildren>().windows.clone();
    for handle in shaded {
        let alive = handle
            .update(cx, |root, window, cx| {
                // Everything in meta is the workspace's except the cursor,
                // which is per window: the pointer in a popped-out panel
                // never moves the workspace's, so a child running the same
                // shader would fade out with the hand right on it.
                let mut meta = meta;
                meta[6] = panel::shader::cursor_presence(window);
                window.set_post_signals(signals, meta);
                cx.notify(root.entity_id());
            })
            .is_ok();
        if !alive {
            cx.default_global::<ShadedChildren>()
                .windows
                .retain(|h| *h != handle);
        }
    }
}

/// The idle half of [`push_child_signals`]: move each shaded child's mouse
/// and keep its frames coming without pushing signals, so a paused lamp
/// tracks the cursor there too while the clocks stay parked.
fn push_child_mouse(cx: &mut App) {
    let shaded = cx.default_global::<ShadedChildren>().windows.clone();
    for handle in shaded {
        let alive = handle
            .update(cx, |root, window, cx| {
                window.set_post_mouse();
                cx.notify(root.entity_id());
            })
            .is_ok();
        if !alive {
            cx.default_global::<ShadedChildren>()
                .windows
                .retain(|h| *h != handle);
        }
    }
}

/// This frame's slot values for the screen shader, the app-wide twin of a
/// panel surface's own resolve.
///
/// Two ways in, and which one runs is decided by whether anything has been
/// routed. With routes, they resolve into slots exactly like a panel's do.
/// With none, the pool fills the slots in its own order. That's the
/// behaviour from before the routes existed, kept because a setup tuned
/// against it would otherwise go dark on upgrade. The first route someone
/// adds takes over the whole feed, which is the only reading that doesn't
/// have two things writing the same slot.
fn post_shader_signals(hub: &SignalHub) -> [f32; panel::shader::SLOTS] {
    // Hand-set values go down first and whatever fills a slot writes over
    // them, which is exactly the panel rule: a route wins while it's
    // there, the hand-set value comes back when it goes. The legacy pool
    // feed steps around hand-set slots for the same reason a route list
    // doesn't: both are somebody explicitly claiming the slot.
    let manual = POST_SHADER_MANUAL.read().unwrap().clone();
    let mut targets = panel::shader::SlotTargets::default();
    for (slot, value) in &manual {
        if let Some(entry) = targets.slots.get_mut(*slot as usize) {
            *entry = *value;
        }
    }
    let routes = POST_SHADER_ROUTES.read().unwrap();
    if routes.is_empty() {
        for (slot, signal) in hub.pool().iter().take(targets.slots.len()).enumerate() {
            if manual.iter().any(|(at, _)| *at as usize == slot) {
                continue;
            }
            targets.slots[slot] = hub.value(signal.id).unwrap_or(0.0);
        }
        return targets.slots;
    }
    rox_panel_api::signal_ui::apply_routes(&routes, hub, &mut targets);
    targets.slots
}

/// The per-window side of the post shader (the Shader settings page's Screen
/// Shader section): which file this window runs and its stamp, for the
/// render loop's hot reload.
struct PostShaderDriver {
    /// The file the shader was read from, None when it came from the pool
    /// or from a bundle's inline copy. Those have nothing to watch, but the
    /// driver still exists for them: it's also what pushes the shader its
    /// signals and keeps the frames coming.
    path: Option<PathBuf>,
    /// The file's stamp when it was last read, [`settings::file_stamp`]'s
    /// size and mtime. A change re-reads; None (unreadable) counts as a
    /// change too, so a file swapped back into place reloads.
    stamp: Option<(u64, i64)>,
    /// The last stat, so the hot-reload check costs one syscall a second
    /// rather than one per frame.
    checked: Instant,
    /// Whether a compiled shader is actually installed on the window. A
    /// compile error leaves the last good shader running, so this stays
    /// true through a broken edit and false only until the first success.
    active: bool,
    /// Whether the hub was live on the last look, so the frame after the
    /// music stops pushes one final update instead of freezing the
    /// uniforms mid-song. Starts true: the first update after an apply
    /// delivers signals and meta once even into a silent app.
    was_live: bool,
    /// The meta floats the last push sent. A parked hub still pushes
    /// when these move, so a theme swap or an easing art tint gets to the
    /// pass while the music is paused instead of waiting for it.
    meta: [f32; 8],
    /// The config's idle switch, held here so the frame loop doesn't read
    /// the settings file per frame. Frames keep coming while the audio is
    /// silent; the uniforms don't move, so only per-draw state (the mouse)
    /// follows along.
    run_when_idle: bool,
    /// Whether the program binds `@cover`, so the frame loop only watches
    /// the cover feed for a shader that asked for the art.
    uses_cover: bool,
    /// Whether the program reads the pointer, so only a shader that does
    /// keeps frames coming while cursor presence eases off.
    reads_cursor: bool,
    /// The cover feed's revision the program registered with. The panel
    /// surfaces re-key per frame and follow the track by themselves; this
    /// pass registers on apply, so a moved rev re-applies it.
    cover: u64,
}

/// Tear down a workspace window's app-level state: persist its layout, drop
/// its player's art tint, forget the window, and quit once the last one
/// goes. The OS close runs this from `on_window_should_close`; the Window
/// Controls close button runs it too before removing the window, since a
/// programmatic `remove_window` never fires the OS hook. `workspace` is None
/// only when the entity has already gone.
///
/// With quit-to-tray on and a way back in resident (the tray icon, the dock
/// on macOS), the last close hands the state to [`tray::hold`] instead of
/// quitting: playback keeps going and the tray's Open adopts the same state
/// into a fresh window. The art tint stays too, so the reopened window comes
/// back themed without waiting for a track change.
pub(crate) fn close_workspace_window(
    workspace: Option<Entity<Workspace>>,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    let open = cx.default_global::<WorkspaceWindows>();
    open.open.retain(|w| w.handle != handle);
    let last = open.open.is_empty();
    let stay = last && settings::quit_to_tray() && tray::resident(cx) && workspace.is_some();
    let mut media = None;
    if let Some(ws) = workspace {
        let player = ws.read(cx).state.player.entity_id();
        let state = stay.then(|| ws.read(cx).state.clone());
        ws.update(cx, |this, cx| {
            this.persist(window, cx);
            // Take the OS media service off the window before a survivor
            // re-registers it; the D-Bus name is per-process, so both can't
            // hold it at once.
            media = this.take_media();
        });
        match state {
            // Going to the tray keeps the service running where the platform
            // allows one with no window behind it, so the media keys still
            // work while the app is only an icon. Windows' SMTC is bound to
            // the window handle it registered against, so there it goes down
            // with the window and the reopen registers a fresh one.
            Some(state) => tray::hold(
                state,
                media.take().filter(|_| !cfg!(target_os = "windows")),
                cx,
            ),
            // A shared pop-out re-seeds on its next track change, so a
            // stale entry never lingers.
            None => palette::forget(player, cx),
        }
    }
    // Whatever the tray didn't take ends here, which frees the per-process
    // name before the hand-off below claims it again.
    let had_media = media.take().is_some();
    // The window that owned the media service just closed with others still
    // open; hand the service to a survivor so the media keys keep working.
    // Each window's service is bound to its own player, so the survivor
    // registers anew rather than inheriting this one.
    if had_media && !last {
        if let Some((handle, ws)) = cx
            .default_global::<WorkspaceWindows>()
            .open
            .first()
            .map(|w| (w.handle, typed_workspace(&w.workspace)))
        {
            let _ = handle.update(cx, |_, window, cx| {
                if let Some(ws) = ws {
                    ws.update(cx, |this, cx| this.install_media(window, cx));
                }
            });
        }
    }
    // Closing the last workspace window quits; without this, a settings or
    // popout window left open keeps the app running with the menubar (and
    // New Window) gone.
    if last && !stay {
        cx.quit();
    }
}

/// Close the focused window for the Cmd/Ctrl+W chord. A child window
/// (settings, stats, about, a popped-out panel) just closes, as does any
/// workspace window with others still open. The chord is a soft close, never
/// a quit: on the last workspace window it only acts when tray mode can catch
/// the app, so Cmd+W sends it to the tray instead of quitting it out from
/// under a reflex. With tray off it's a no-op there: use the menu or Cmd+Q.
fn close_active_window(cx: &mut App) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    // Defer out of key dispatch: the action fires while this window is mid
    // update (taken out of the slot), so updating it again from here is
    // refused. Running a tick later, once the slot is back, lets it through.
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            if is_workspace_window(window, cx) {
                let last = cx.default_global::<WorkspaceWindows>().open.len() <= 1;
                let would_quit = last && !(settings::quit_to_tray() && tray::resident(cx));
                if !would_quit {
                    let workspace = workspace_for_window(window, cx).and_then(|ws| ws.upgrade());
                    close_workspace_window(workspace, window, cx);
                    window.remove_window();
                }
            } else {
                window.remove_window();
            }
        });
    });
}

/// Route OS-handed files into a workspace's player: the command line at
/// launch, and the files a second launch forwards through the single-instance
/// guard. Play replaces what's loaded, enqueue appends to the up-next queue.
pub(crate) fn play_launch_paths(
    state: &AppState,
    mode: rox_library::open_files::LaunchMode,
    paths: Vec<PathBuf>,
    cx: &mut App,
) {
    if paths.is_empty() {
        return;
    }
    // Files handed over by the OS are whole files: nothing out there names a
    // subsong, so they queue as plain tracks.
    let keys: Vec<TrackKey> = paths.into_iter().map(TrackKey::from).collect();
    state.player.update(cx, |player, cx| match mode {
        rox_library::open_files::LaunchMode::Play => player.play(keys, cx),
        rox_library::open_files::LaunchMode::Enqueue => player.enqueue(keys, cx),
    });
}

/// Filter dropped paths to decodable audio and read them as whole files.
/// A drop off the desktop names files, never subsongs; a drag out of the
/// library brings its own keys and never comes through here.
fn loose_keys(paths: Vec<PathBuf>) -> Vec<TrackKey> {
    rox_library::open_files::resolve_audio_paths(paths)
        .into_iter()
        .map(TrackKey::from)
        .collect()
}

/// Whether the caller already told the user a screen shader was coming.
/// The apply confirms name the shader and the hotkey that turns it off, so
/// the keep-or-revert window after the fact would be the same warning
/// twice. The welcome window's tiles apply straight off a click with
/// nothing in between, so there that window is the only thing that says the
/// look brought one, and it's also the way back out of a look whose menubar
/// is hidden.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShaderNotice {
    /// A confirm already read the shader out. Apply it and say nothing.
    Told,
    /// Nothing did. Open the keep-or-revert window over the fresh look.
    Ask,
}

/// Which of the apply confirm's two yeses ran, for a look that brings
/// shaders. Every apply of such a look asks, not just the first one: what a
/// shader does to a look is a matter of taste, and taste is allowed to change
/// between two applies of the same workspace.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyShaders {
    /// Wear what the look brought, agreeing to any of it this machine hasn't
    /// met before.
    Wear,
    /// Apply the look bare: no overlay, no panel with a shader on it. The
    /// shader pool still travels, so anything the bundle named is a picker
    /// click away.
    Skip,
}

/// Apply a named workspace to the frontmost workspace window from app
/// level, the same path the settings window's Apply takes. The welcome
/// window's quick-start tiles come through here: they're in their own OS
/// window, with no workspace of their own to call into.
pub(crate) fn apply_workspace_to_front(name: &str, cx: &mut App) {
    let found = cx
        .default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find_map(|w| Some((w.handle, typed_workspace(&w.workspace)?)));
    if let Some((handle, ws)) = found {
        let name = name.to_string();
        let _ = handle.update(cx, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.apply_workspace(&name, ApplyShaders::Wear, ShaderNotice::Ask, window, cx)
            });
        });
    }
}

/// Whether `window` is one of the tracked workspace windows, told apart
/// from settings, popouts, and editors. The Window Controls close button
/// only runs the workspace teardown for these.
pub(crate) fn is_workspace_window(window: &Window, cx: &mut App) -> bool {
    let handle = window.window_handle();
    cx.default_global::<WorkspaceWindows>()
        .open
        .iter()
        .any(|w| w.handle == handle)
}

/// The workspace hosting `window`, when it's a workspace window (not a
/// popout, settings, or editor). The queue widget uses it to reach the
/// workspace and open the queue modal there.
pub(crate) fn workspace_for_window(window: &Window, cx: &App) -> Option<WeakEntity<Workspace>> {
    let handle = window.window_handle();
    let entry = cx
        .try_global::<WorkspaceWindows>()?
        .open
        .iter()
        .find(|w| w.handle == handle)?;
    Some(typed_workspace(&entry.workspace)?.downgrade())
}

/// Append the "Add Panel" flyout to a panel's dropdown as its own section:
/// the whole catalog as a submenu, every group (Application, Arrangement,
/// Controls, Catalogue, Details, Visualizers) as its own nested flyout. A
/// pick
/// opens the panel as a new tab of `tab_panel`, that very group, skipping
/// the placement rules the menubar routes follow. Built as a real submenu
/// the way [`rox_panel_api::query::shared_query::search_flyout`] builds its
/// Search flyout (a hand-built menu entity behind a submenu item), so it
/// works from every host of the panel menu, the content context menus
/// included. Leads with a divider so it reads as its own band rather than
/// the tail of whatever content section is above it; the separator is a
/// no-op when Add Panel would be the menu's first item. A popped-out panel
/// (no group) or a window with no workspace behind it gets nothing.
pub(crate) fn add_panel_submenu(
    menu: PopupMenu,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let Some(tabs) = tab_panel else {
        return menu;
    };
    let handle = window.window_handle();
    let Some(entity) = cx
        .default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find(|w| w.handle == handle)
        .and_then(|w| typed_workspace(&w.workspace))
    else {
        return menu;
    };
    let dock = entity.read(cx).dock.downgrade();
    let workspace = entity.downgrade();
    let submenu = PopupMenu::build(window, cx, move |mut menu, window, cx| {
        // The saved panels lead: a preset is a panel you already decided on,
        // so it goes above the catalog it was built out of.
        let tabs_for_preset = tabs.clone();
        menu = panel_presets::pick_submenu(
            menu,
            dock.clone(),
            false,
            window,
            cx,
            move |panel, window, cx| {
                if let Some(tabs) = tabs_for_preset.upgrade() {
                    tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
                }
            },
        );
        for section in catalog::sections() {
            match section.group {
                None => {
                    for def in section.panels {
                        menu = add_panel_item(menu, def, workspace.clone(), tabs.clone());
                    }
                }
                Some((label, icon)) => {
                    let workspace = workspace.clone();
                    let tabs = tabs.clone();
                    menu = menu.submenu_with_icon(
                        Some(Icon::default().path(icon)),
                        rox_i18n::t!(label),
                        window,
                        cx,
                        move |mut menu, _, _| {
                            for def in section.panels {
                                menu = add_panel_item(menu, def, workspace.clone(), tabs.clone());
                            }
                            menu
                        },
                    );
                }
            }
        }
        menu
    });
    menu.separator().item(
        gpui_component::menu::PopupMenuItem::submenu(
            rox_i18n::t!("workspace-context-add-panel"),
            submenu,
        )
        .icon(Icon::default().path(icons::PLUS)),
    )
}

/// One Add Panel row: build the def's panel against the workspace's state
/// and add it as a tab of the clicked group.
fn add_panel_item(
    menu: PopupMenu,
    def: &'static PanelDef,
    workspace: WeakEntity<Workspace>,
    tabs: WeakEntity<TabPanel>,
) -> PopupMenu {
    // A panel the signal pool can drive trails the glyph, which a plain
    // item has no room for, so those rows render their own line.
    let item = if catalog::supports_signals(def) {
        gpui_component::menu::PopupMenuItem::element(move |_, _| {
            div()
                .flex()
                .flex_1()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(div().flex_1().child(rox_i18n::t!(def.label)))
                .child(
                    svg()
                        .path(icons::AUDIO_WAVEFORM)
                        .size_3()
                        .text_color(palette::text_faint()),
                )
        })
    } else {
        gpui_component::menu::PopupMenuItem::new(rox_i18n::t!(def.label))
    };
    menu.item(
        item.icon(Icon::default().path(def.icon))
            .on_click(move |_, window, cx| {
                let (Some(ws), Some(tabs)) = (workspace.upgrade(), tabs.upgrade()) else {
                    return;
                };
                let state = ws.read(cx).state.clone();
                let panel = (def.build)(&state, workspace.clone(), window, cx);
                tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
            }),
    )
}

/// Versions the layout dump in settings. Bump on incompatible panel or
/// schema changes; a dump from another version is ignored and the default
/// layout builds instead.
const LAYOUT_VERSION: usize = 1;

/// Layout events fire for every step of a drag or resize, so a save waits
/// out this much quiet first. The close hook catches whatever a pending
/// debounce still holds.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// The transport row's starting height, just enough for the controls plus
/// their status line. The row is a regular split in the one layout tree,
/// so it resizes and collapses like everything else.
const TRANSPORT_ROW_H: f32 = 120.0;

/// The search bar's starting height: the 30px tab strip plus its one input
/// row. It joins as a thin strip, not a tall tile, so it opens near its
/// minimum instead of taking a center panel's share.
const SEARCH_BAR_H: f32 = 52.0;

// The three playback actions panels bind against are in rox-panel-api, so
// a panel's transport button and this window's keymap name the same type.
// Registration and the handlers stay here.
use rox_panel_api::actions::{SeekBackward, SeekForward, TogglePlayback};

actions!(
    rox,
    [
        OpenSettings,
        OpenStats,
        OpenQuickPlay,
        FocusSearch,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
        TogglePostShader,
        CloseWindow,
        Quit
    ]
);

/// The app-level action handlers, above every window; call once at
/// startup. The chords that reach them are the keymap module's, built
/// from the settings file rather than written out here.
pub fn init(cx: &mut App) {
    // Fallback for windows without a workspace in the focus path (popped-out
    // panels); workspace windows persist their layout first via their own
    // handler, which stops the action before it gets here.
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &CloseWindow, cx| close_active_window(cx));
    // Every native-menu row that isn't one of the bound actions below comes
    // through here. Nothing binds or handles MenuCommand in a window, so the
    // global is the only stop.
    cx.on_action(|command: &MenuCommand, cx| run_menu_command(&command.command, cx));
    // The bound actions the native menu emits directly, so it can draw their
    // shortcut. A workspace window handles them itself and stops the action
    // during the bubble; these catch the case where a child window (settings,
    // stats, a popped-out panel) is key, which would otherwise swallow the
    // pick, and route it to the front workspace like every other row.
    cx.on_action(|_: &TogglePlayback, cx| {
        with_front_workspace(cx, |ws, _, cx| {
            ws.state
                .player
                .update(cx, |player, _| player.toggle_pause());
        });
    });
    cx.on_action(|_: &OpenSettings, cx| {
        with_front_workspace(cx, |ws, window, cx| {
            crate::settings::window::open(
                ws.state.clone(),
                cx.entity().downgrade(),
                window.window_handle(),
                ws.dock.clone(),
                cx,
            );
        });
    });
    cx.on_action(|_: &OpenStats, cx| {
        with_front_workspace(cx, |ws, _, cx| {
            crate::stats_window::open(ws.state.clone(), cx);
        });
    });
    // The zoom chords are app-wide, so they hang off global handlers rather
    // than any one window's view: whichever window has focus dispatches, and
    // the size setter repaints them all.
    cx.on_action(|_: &IncreaseFontSize, cx| nudge_font_size(1.0, cx));
    cx.on_action(|_: &DecreaseFontSize, cx| nudge_font_size(-1.0, cx));
    cx.on_action(|_: &ResetFontSize, cx| set_font_size(palette::FONT_SIZE_DEFAULT, cx));
    // The shader flip is app-wide like the zoom chords: whichever window
    // has focus dispatches, and the apply covers them all.
    cx.on_action(|_: &TogglePostShader, cx| toggle_post_shader(cx));
}

/// Nudge the app font size by `delta` px from where it stands.
fn nudge_font_size(delta: f32, cx: &mut App) {
    set_font_size(palette::app_font_size() + delta, cx);
}

/// Set the app font size and persist it. The live setter clamps to the
/// palette's range and repaints every window; the settings write keeps the
/// new size across launches, the same pair the settings slider drives. A
/// user-level readability choice, so it's stored on `Settings` directly and
/// persists across workspace switches, like the theme pick.
fn set_font_size(size: f32, cx: &mut App) {
    let size = size.clamp(palette::FONT_SIZE_MIN, palette::FONT_SIZE_MAX);
    palette::set_app_font_size(size, cx);
    Settings::update(move |s| s.app_font_size = size);
    // The setter's repaint loop gets to every window but the one dispatching
    // this action: we're mid-update inside it, so its re-entrant refresh is
    // dropped and the resize would wait until the next input event. Defer a
    // second pass that runs once this update has finished, so the focused
    // window wakes now too.
    cx.defer(|cx| {
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// Teach the dock's registry to rebuild every panel type from a layout
/// dump. Registered per workspace so the builders capture that workspace's
/// entities; the restore runs synchronously right after, before another
/// workspace can re-register. The workspace handle is for the panels that
/// drive it back: the window controls' mini toggle and the menu panel.
fn register_panels(state: &AppState, workspace: WeakEntity<Workspace>, cx: &mut App) {
    let s = state.clone();
    register_panel(cx, "library", move |_, _, info, window, cx| {
        let config: LibraryConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| LibraryPanel::new(s.clone(), config, window, cx)))
    });
    // A panel whose config is stored in the layout dump.
    macro_rules! configured {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, info, _, cx| {
                let config = panel::config_from_info(info);
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), config, cx)))
            });
        }};
    }
    // The same, but the constructor takes a window: a searching panel spins up
    // its search box's input state at build, like the library's and grid's.
    macro_rules! configured_windowed {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            register_panel(cx, $name, move |_, _, info, window, cx| {
                let config = panel::config_from_info(info);
                Box::new(cx.new(|cx| <$panel>::new(s.clone(), config, window, cx)))
            });
        }};
    }
    // Filter takes a window at build so its quick-search box can spin up
    // an input state, like the library panel's search.
    let s = state.clone();
    register_panel(cx, "filter", move |_, _, info, window, cx| {
        let config: FilterConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| FilterPanel::new(s.clone(), config, window, cx)))
    });
    // The folder tree takes a window at build to match its constructor,
    // like the filter's.
    let s = state.clone();
    register_panel(cx, "folder tree", move |_, _, info, window, cx| {
        let config = panel::config_from_info(info);
        Box::new(cx.new(|cx| FolderTreePanel::new(s.clone(), config, window, cx)))
    });
    configured!("seek", SeekStripPanel);
    configured!("track info", TrackInfoPanel);
    configured!("status", StatusPanel);
    configured!("cover art", CoverArtPanel);
    configured!("metadata", MetadataPanel);
    configured!("lyrics", LyricsPanel);
    configured!("biography", BiographyPanel);
    configured!("output", OutputPanel);
    configured_windowed!("history", HistoryPanel);
    configured_windowed!("queue", QueuePanel);
    configured!("queue widget", QueueWidgetPanel);
    configured!("eq widget", EqWidgetPanel);
    configured!("stats widget", StatsWidgetPanel);
    // The retired standalone rating and favourite panels: their names
    // restore as track info cards holding the one piece, so a layout
    // saved with them keeps its stars and heart. The chrome (caps, theme,
    // locks) reads straight across, both configs flatten the same shape;
    // the next save writes the card, and the migration is done. [compat]
    for (name, piece) in [
        ("rating", rox_panels::transport::InfoPiece::Rating),
        ("favourite", rox_panels::transport::InfoPiece::Favourite),
    ] {
        let s = state.clone();
        register_panel(cx, name, move |_, _, info, _, cx| {
            let mut config: rox_panels::transport::TrackInfoConfig = panel::config_from_info(info);
            config.items = vec![piece];
            Box::new(cx.new(|cx| TrackInfoPanel::new(s.clone(), config, cx)))
        });
    }
    configured_windowed!("playlists", PlaylistsPanel);
    // The composition hosts rebuild their children through this same
    // registry, and take the workspace handle so their slot menus can
    // build replacements from the catalog.
    macro_rules! composite {
        ($name:literal, $panel:ty) => {{
            let s = state.clone();
            let ws = workspace.clone();
            register_panel(
                cx,
                $name,
                move |dock_area, panel_state, info, window, cx| {
                    let config = panel::config_from_info(info);
                    let slots = composite::restore_slots(&dock_area, panel_state, window, cx);
                    Box::new(
                        cx.new(|cx| <$panel>::restore(s.clone(), ws.clone(), config, slots, cx)),
                    )
                },
            );
        }};
    }
    composite!("drawer", DrawerPanel);
    composite!("group", GroupPanel);
    composite!("overlay", OverlayPanel);
    // "depth" was this panel's old name; keep the alias so layouts saved
    // before the rename still rebuild it.
    composite!("depth", OverlayPanel);
    composite!("slide", SlidePanel);
    // The grid takes the window like the library: its search box builds
    // an input state.
    let s = state.clone();
    register_panel(cx, "album grid", move |_, _, info, window, cx| {
        let config: GridConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| GridPanel::new(s.clone(), config, window, cx)))
    });
    // The artist wall has a search box too, so it takes the window like
    // the album grid.
    let s = state.clone();
    register_panel(cx, "artist grid", move |_, _, info, window, cx| {
        let config: ArtistGridConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| ArtistGridPanel::new(s.clone(), config, window, cx)))
    });
    // The genre wall, the artist grid's sibling over the "; " lists.
    let s = state.clone();
    register_panel(cx, "genre grid", move |_, _, info, window, cx| {
        let config: GenreGridConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| GenreGridPanel::new(s.clone(), config, window, cx)))
    });
    // The art strip shares the grid's search box, so it takes the window
    // the same way.
    let s = state.clone();
    register_panel(cx, "art view", move |_, _, info, window, cx| {
        let config: ArtConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| ArtPanel::new(s.clone(), config, window, cx)))
    });
    // The search panel builds its box's input state, so it takes the window
    // like the library.
    let s = state.clone();
    register_panel(cx, "search", move |_, _, info, window, cx| {
        let config: SearchConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| SearchPanel::new(s.clone(), config, window, cx)))
    });
    configured!("playback", TransportPanel);
    configured!("volume", VolumePanel);
    configured!("spectrum", SpectrumPanel);
    configured!("spectrogram", SpectrogramPanel);
    configured!("oscilloscope", OscilloscopePanel);
    configured!("waveform", WaveformPanel);
    configured!("vu meter", VuPanel);
    // Registered whether or not experimental features are on: the flag
    // gates the panel menus, not a layout that already holds one.
    configured!("particles", ParticlesPanel);
    configured!("shader", ShaderPanel);
    configured!("drag anchor", DragAnchorPanel);
    configured!("spacer", SpacerPanel);
    configured!("theme toggle", ThemeTogglePanel);
    // These two drive the workspace back, so their builders take its
    // handle alongside the shared state.
    let s = state.clone();
    let ws = workspace.clone();
    register_panel(cx, "window controls", move |_, _, info, _, cx| {
        let config: WindowControlsConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| WindowControlsPanel::new(s.clone(), ws.clone(), config, cx)))
    });
    let s = state.clone();
    let ws = workspace.clone();
    register_panel(cx, "mini toggle", move |_, _, info, _, cx| {
        let config: MiniToggleConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| MiniTogglePanel::new(s.clone(), ws.clone(), config, cx)))
    });
    let s = state.clone();
    register_panel(cx, "menu", move |_, _, info, _, cx| {
        let config: MenuConfig = panel::config_from_info(info);
        Box::new(cx.new(|cx| MenuPanel::new(s.clone(), workspace.clone(), config, cx)))
    });
}

#[derive(Clone, Copy)]
pub(crate) enum MenuAction {
    NewWindow,
    EmptyWindow,
    /// Toggle play/pause on the current track. Labeled "Play"; the trailing
    /// Space shortcut matches the global [`TogglePlayback`] binding.
    TogglePlayback,
    Stop,
    Next,
    Previous,
    OpenSettings,
    OpenStats,
    OpenConsole,
    OpenTasks,
    OpenEqualizer,
    /// Open the shared signal pool, the window behind every panel's routes.
    OpenSignals,
    OpenWelcome,
    OpenAbout,
    /// The three links out to the project, each opening in the browser:
    /// the issue form, the discussions board, and the IRC channel.
    ReportIssue,
    OpenDiscussions,
    OpenChat,
    ToggleMenubar,
    ToggleDecorations,
    /// Flip design mode: whether the layout can be rearranged in place, or
    /// reads as finished furniture.
    ToggleDesignMode,
    /// Flip song theming: the playing track's art tinting the palette.
    ToggleArtTheming,
    /// Flip the screen shader, the hotkey's menu twin. Never prompts.
    TogglePostShader,
    /// Pick a workspace file and add it to the collection.
    ImportWorkspace,
    /// Open a catalog panel with its default config, opening where its
    /// placement says. One action for every panel in the catalog.
    OpenPanel(&'static PanelDef),
    ToggleQuitToTray,
    CloseWindow,
    Quit,
}

#[derive(Clone, Copy)]
pub(crate) struct MenuItem {
    /// An i18n key, not display text. `MENUS` is `const`, and a translator
    /// call isn't, so every renderer resolves this through
    /// [`menu_item_display`] (or, off the shared path, `t_static`
    /// directly) rather than showing it as-is. A panel row is the one
    /// exception: [`panel_menu_item`] fills this from [`PanelDef::label`],
    /// which is a catalog identifier, not one of ours.
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) action: MenuAction,
}

/// A native-menu pick, encoded as a string so one action covers the whole
/// menu tree. The macOS system bar dispatches actions rather than calling
/// our handlers, and only a registered `Action` can pass through it, so
/// every row that isn't already a bound action (Play, Settings, Stats,
/// Quit) encodes itself here and [`run_menu_command`] decodes it back.
/// See [`MenuAction::command_id`] for the encoding. `no_json` because this
/// is only ever built in code, never named in a keymap file, which is what
/// the JSON path is for. It also keeps schemars out of our dependencies.
#[derive(Clone, PartialEq, Eq, gpui::Action)]
#[action(namespace = rox, no_json)]
pub(crate) struct MenuCommand {
    pub(crate) command: String,
}

impl MenuAction {
    /// This action as a [`MenuCommand`] payload, or None for the four that
    /// go through their own bound action so the native menu can draw their
    /// shortcut. Panels encode by label, which the catalog keys on. Only the
    /// native bar encodes, and that's macOS-only, so off macOS this is dead.
    #[cfg(target_os = "macos")]
    pub(crate) fn command_id(self) -> Option<String> {
        let id = match self {
            // These go through their own actions, for the shortcut.
            MenuAction::TogglePlayback
            | MenuAction::OpenSettings
            | MenuAction::OpenStats
            | MenuAction::Quit => return None,
            MenuAction::NewWindow => "new-window".into(),
            MenuAction::EmptyWindow => "empty-window".into(),
            MenuAction::Stop => "stop".into(),
            MenuAction::Next => "next".into(),
            MenuAction::Previous => "previous".into(),
            MenuAction::OpenConsole => "console".into(),
            MenuAction::OpenTasks => "tasks".into(),
            MenuAction::OpenEqualizer => "equalizer".into(),
            MenuAction::OpenSignals => "signals".into(),
            MenuAction::OpenWelcome => "welcome".into(),
            MenuAction::OpenAbout => "about".into(),
            MenuAction::ReportIssue => "report-issue".into(),
            MenuAction::OpenDiscussions => "discussions".into(),
            MenuAction::OpenChat => "chat".into(),
            MenuAction::ToggleMenubar => "toggle-menubar".into(),
            MenuAction::ToggleDecorations => "toggle-decorations".into(),
            MenuAction::ToggleDesignMode => "toggle-design-mode".into(),
            MenuAction::ToggleArtTheming => "toggle-art-theming".into(),
            MenuAction::TogglePostShader => "toggle-post-shader".into(),
            MenuAction::ImportWorkspace => "import-workspace".into(),
            MenuAction::ToggleQuitToTray => "toggle-quit-to-tray".into(),
            MenuAction::CloseWindow => "close-window".into(),
            // Keyed by registry name. The label is an i18n key that a
            // translator is free to reword, so encoding identity in it
            // means the round trip stops resolving the moment one does.
            MenuAction::OpenPanel(def) => format!("panel:{}", def.name),
        };
        Some(id)
    }

    /// The inverse of [`MenuAction::command_id`]. A panel resolves through
    /// the catalog, so a row for a panel that has since been gated off
    /// (experimental) decodes to None and the pick does nothing.
    fn from_command_id(id: &str) -> Option<MenuAction> {
        if let Some(name) = id.strip_prefix("panel:") {
            return catalog::sections()
                .flat_map(|section| section.panels.iter())
                .find(|def| def.name == name)
                .map(MenuAction::OpenPanel);
        }
        Some(match id {
            "new-window" => MenuAction::NewWindow,
            "empty-window" => MenuAction::EmptyWindow,
            "stop" => MenuAction::Stop,
            "next" => MenuAction::Next,
            "previous" => MenuAction::Previous,
            "console" => MenuAction::OpenConsole,
            "tasks" => MenuAction::OpenTasks,
            "equalizer" => MenuAction::OpenEqualizer,
            "signals" => MenuAction::OpenSignals,
            "welcome" => MenuAction::OpenWelcome,
            "about" => MenuAction::OpenAbout,
            "report-issue" => MenuAction::ReportIssue,
            "discussions" => MenuAction::OpenDiscussions,
            "chat" => MenuAction::OpenChat,
            "toggle-menubar" => MenuAction::ToggleMenubar,
            "toggle-decorations" => MenuAction::ToggleDecorations,
            "toggle-art-theming" => MenuAction::ToggleArtTheming,
            "toggle-post-shader" => MenuAction::TogglePostShader,
            "import-workspace" => MenuAction::ImportWorkspace,
            "toggle-quit-to-tray" => MenuAction::ToggleQuitToTray,
            "close-window" => MenuAction::CloseWindow,
            _ => return None,
        })
    }
}

/// The workspace a native-menu pick drives: the frontmost one, with its
/// window. The system bar belongs to the app rather than any one window, and
/// it stays reachable while a child window (settings, stats, a popped-out
/// panel) has focus, so picks route here instead of at the key window.
fn front_workspace_entity(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    cx.default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find_map(|w| Some((w.handle, typed_workspace(&w.workspace)?)))
}

/// Run `f` against the frontmost workspace inside its own window. Deferred:
/// a menu pick can arrive while that window is mid-update (its own render
/// dispatched the action), and updating a window already on the update stack
/// is refused, the same reason [`close_active_window`] defers.
fn with_front_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) {
    cx.defer(move |cx| {
        let Some((handle, workspace)) = front_workspace_entity(cx) else {
            return;
        };
        let _ = handle.update(cx, |_, window, cx| {
            workspace.update(cx, |ws, cx| f(ws, window, cx));
        });
    });
}

/// Run a native-menu pick. The plain rows decode back to a [`MenuAction`]
/// and go through the same [`Workspace::run`] the in-window bar uses; the
/// workspace and layout rows have a name in them, so they route to the same
/// flyout handlers those rows call directly.
fn run_menu_command(command: &str, cx: &mut App) {
    let Some((kind, name)) = command.split_once(':') else {
        // The nameless rows: a plain action, or a save dialog.
        match command {
            "workspace-save-new" => {
                with_front_workspace(cx, |ws, window, cx| {
                    ws.open_save_workspace_dialog(window, cx)
                });
            }
            "layout-save-new" => {
                with_front_workspace(cx, |ws, window, cx| ws.open_save_dialog(window, cx));
            }
            _ => {
                if let Some(action) = MenuAction::from_command_id(command) {
                    with_front_workspace(cx, move |ws, window, cx| ws.run(action, window, cx));
                }
            }
        }
        return;
    };
    let name = name.to_string();
    match kind {
        "workspace-apply" => with_front_workspace(cx, move |ws, _, cx| {
            ws.run_workspace(name, WorkspaceTarget::Apply, cx)
        }),
        "workspace-save" => with_front_workspace(cx, move |ws, _, cx| {
            ws.run_workspace(name, WorkspaceTarget::Overwrite, cx)
        }),
        "layout-new" => with_front_workspace(cx, move |ws, _, cx| {
            ws.run_layout(name, LayoutTarget::NewWindow, cx)
        }),
        "layout-save" => with_front_workspace(cx, move |ws, _, cx| {
            ws.run_layout(name, LayoutTarget::Overwrite, cx)
        }),
        "layout-apply" => with_front_workspace(cx, move |ws, _, cx| {
            ws.run_layout(name, LayoutTarget::Apply, cx)
        }),
        "panel-preset" => with_front_workspace(cx, move |ws, window, cx| {
            ws.run_panel_preset(name, PanelTarget::Open, window, cx)
        }),
        "panel-preset-window" => with_front_workspace(cx, move |ws, window, cx| {
            ws.run_panel_preset(name, PanelTarget::NewWindow, window, cx)
        }),
        // A catalog panel straight into a window of its own, the Window
        // menu's half of the same flyout. Keyed by registry name like
        // "panel:".
        "panel-window" => {
            if let Some(def) = catalog::sections()
                .flat_map(|section| section.panels.iter())
                .find(|def| def.name == name)
            {
                with_front_workspace(cx, move |ws, window, cx| {
                    ws.open_panel_window(def, window, cx)
                });
            }
        }
        // "panel:<label>" and anything else with a colon in it.
        _ => {
            if let Some(action) = MenuAction::from_command_id(command) {
                with_front_workspace(cx, move |ws, window, cx| ws.run(action, window, cx));
            }
        }
    }
}

/// A catalog entry as a dropdown row, for the renderers that show panel
/// sections: the def's own label and icon over its open action.
pub(crate) fn panel_menu_item(def: &'static PanelDef) -> MenuItem {
    MenuItem {
        label: def.label,
        icon: def.icon,
        action: MenuAction::OpenPanel(def),
    }
}

/// What picking a preset in a layouts flyout does.
#[derive(Clone, Copy)]
pub(crate) enum LayoutTarget {
    /// Open a fresh window built from the preset.
    NewWindow,
    /// Replace the preset with the current arrangement.
    Overwrite,
    /// Swap the current window into the preset, after a confirm.
    Apply,
}

/// What picking a panel in a presets or panel flyout does: put it in this
/// window's layout, or open it in a window of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelTarget {
    Open,
    NewWindow,
}

/// What picking a workspace in a workspaces flyout does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceTarget {
    /// Replace the bundle with the current look. The Save flyout only offers
    /// user bundles, so this never targets a shipped one.
    Overwrite,
    /// Apply the bundle's whole look to this window, after a confirm.
    Apply,
}

/// A dropdown row: an action item, a muted section heading over a divider,
/// a run of panel-catalog entries, or a layout-presets flyout whose items
/// come from the saved presets at open time.
pub(crate) enum MenuEntry {
    Item(MenuItem),
    Section(&'static str),
    /// A catalog section: a bare one renders as plain rows in place, a
    /// labeled group as a submenu that flies out on hover.
    Panels(&'static PanelSection),
    LayoutsSubmenu {
        label: &'static str,
        icon: &'static str,
        target: LayoutTarget,
        /// Lead the flyout with a "New..." row that opens the save dialog,
        /// the Save Layout submenu's way to a fresh preset.
        with_new: bool,
    },
    /// A panel-presets flyout whose items are the saved panels, read at open
    /// time; picking one builds that panel configured and does the flyout's
    /// `target` with it.
    PresetsSubmenu {
        label: &'static str,
        icon: &'static str,
        target: PanelTarget,
    },
    /// The whole panel picker as a two-level flyout: the saved presets, then
    /// the catalog's own groups, each flying out into its panels. Every pick
    /// opens the panel in a window of its own, which is the one thing this
    /// flyout is for.
    PanelWindowsSubmenu {
        label: &'static str,
        icon: &'static str,
    },
    /// A workspaces flyout whose items are the saved and shipped workspaces,
    /// read at open time; picking one does the flyout's `target` with that
    /// bundle behind a confirm.
    WorkspacesSubmenu {
        label: &'static str,
        icon: &'static str,
        target: WorkspaceTarget,
        /// Lead the flyout with a "New..." row that opens the save dialog,
        /// the Save Workspace submenu's way to a fresh bundle.
        with_new: bool,
    },
}

pub(crate) struct Menu {
    /// An i18n key, not display text, same as [`MenuItem::label`]. Every
    /// renderer wraps it in `t!`/`t_static` before showing it.
    pub(crate) label: &'static str,
    pub(crate) entries: &'static [MenuEntry],
}

pub(crate) const MENUS: &[Menu] = &[
    Menu {
        label: "menu-application",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "menu-settings",
                icon: icons::SETTINGS,
                action: MenuAction::OpenSettings,
            }),
            // The two instruments: windows you work while the music plays,
            // rather than preferences you set and close.
            MenuEntry::Section("menu-section-tuning"),
            MenuEntry::Item(MenuItem {
                label: "menu-equalizer",
                icon: icons::AUDIO_LINES,
                action: MenuAction::OpenEqualizer,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-signals",
                icon: icons::AUDIO_WAVEFORM,
                action: MenuAction::OpenSignals,
            }),
            MenuEntry::Section("menu-section-library"),
            MenuEntry::Item(MenuItem {
                label: "menu-stats",
                icon: icons::CHART_PIE,
                action: MenuAction::OpenStats,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-tasks",
                icon: icons::CLOCK,
                action: MenuAction::OpenTasks,
            }),
            MenuEntry::Section("menu-section-app"),
            MenuEntry::Item(MenuItem {
                label: "menu-console",
                icon: icons::FILE_TEXT,
                action: MenuAction::OpenConsole,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-report-issue",
                icon: icons::BUG,
                action: MenuAction::ReportIssue,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-discussions",
                icon: icons::MESSAGES,
                action: MenuAction::OpenDiscussions,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-chat",
                icon: icons::HASH,
                action: MenuAction::OpenChat,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-welcome",
                icon: icons::INFO,
                action: MenuAction::OpenWelcome,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-about",
                icon: icons::LOGO,
                action: MenuAction::OpenAbout,
            }),
            MenuEntry::Section("menu-section-session"),
            MenuEntry::Item(MenuItem {
                label: "menu-exit",
                icon: icons::CLOSE,
                action: MenuAction::Quit,
            }),
        ],
    },
    Menu {
        label: "menu-playback",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "playback-item-play",
                icon: icons::PLAY,
                action: MenuAction::TogglePlayback,
            }),
            MenuEntry::Item(MenuItem {
                label: "playback-item-stop",
                icon: icons::STOP,
                action: MenuAction::Stop,
            }),
            MenuEntry::Section("menu-section-track"),
            MenuEntry::Item(MenuItem {
                label: "playback-item-next",
                icon: icons::SKIP_FORWARD,
                action: MenuAction::Next,
            }),
            MenuEntry::Item(MenuItem {
                label: "playback-item-previous",
                icon: icons::SKIP_BACK,
                action: MenuAction::Previous,
            }),
        ],
    },
    Menu {
        label: "menu-window",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "menu-new-window",
                icon: icons::PLUS,
                action: MenuAction::NewWindow,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-empty-window",
                icon: icons::SQUARE_DASHED,
                action: MenuAction::EmptyWindow,
            }),
            // One panel in a window of its own: a saved preset arrives
            // configured, a catalog pick bare, both the shape a panel dragged
            // out of the dock ends up in.
            MenuEntry::PanelWindowsSubmenu {
                label: "menu-new-window-from-panel",
                icon: icons::EXTERNAL_LINK,
            },
            MenuEntry::Section("menu-section-interface"),
            MenuEntry::Item(MenuItem {
                label: "menu-hide-menubar",
                icon: icons::EYE,
                action: MenuAction::ToggleMenubar,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-os-decorations",
                icon: icons::APP_WINDOW,
                action: MenuAction::ToggleDecorations,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-song-theming",
                icon: icons::DISC,
                action: MenuAction::ToggleArtTheming,
            }),
            MenuEntry::Item(MenuItem {
                label: "menu-overlay-shader",
                icon: icons::BLEND,
                action: MenuAction::TogglePostShader,
            }),
            MenuEntry::Section("menu-section-session"),
            MenuEntry::Item(MenuItem {
                label: "menu-remain-in-tray",
                icon: icons::MINIMIZE,
                action: MenuAction::ToggleQuitToTray,
            }),
            MenuEntry::Item(MenuItem {
                // Closes this window, unlike Application's Exit which quits.
                label: "menu-close",
                icon: icons::CLOSE,
                action: MenuAction::CloseWindow,
            }),
        ],
    },
    Menu {
        label: "menu-workspace",
        entries: &[
            MenuEntry::WorkspacesSubmenu {
                label: "menu-apply-workspace",
                icon: icons::GALLERY,
                target: WorkspaceTarget::Apply,
                with_new: false,
            },
            MenuEntry::WorkspacesSubmenu {
                label: "menu-save-workspace",
                icon: icons::DOWNLOAD,
                target: WorkspaceTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::Item(MenuItem {
                label: "menu-import-workspace",
                icon: icons::UPLOAD,
                action: MenuAction::ImportWorkspace,
            }),
            MenuEntry::Section("menu-section-layouts"),
            MenuEntry::LayoutsSubmenu {
                label: "menu-new-window-from-layout",
                icon: icons::LAYOUT_DASHBOARD,
                target: LayoutTarget::NewWindow,
                with_new: false,
            },
            MenuEntry::LayoutsSubmenu {
                label: "menu-save-layout",
                icon: icons::UPLOAD,
                target: LayoutTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::LayoutsSubmenu {
                label: "menu-apply-layout",
                icon: icons::DOWNLOAD,
                target: LayoutTarget::Apply,
                with_new: false,
            },
        ],
    },
    Menu {
        label: "menu-panels",
        entries: &[
            // The switch over the things it governs: every row under this one
            // puts a panel into the layout, which is what design mode is about,
            // and the menus that arrange panels are the place someone looks for
            // it rather than the Window menu's interface knobs.
            MenuEntry::Item(MenuItem {
                label: "menu-design-mode",
                icon: icons::LAYOUT_DASHBOARD,
                action: MenuAction::ToggleDesignMode,
            }),
            MenuEntry::Section("menu-section-add"),
            // The panels you already configured lead the ones you haven't.
            MenuEntry::PresetsSubmenu {
                label: "menu-panels-presets",
                icon: icons::COPY,
                target: PanelTarget::Open,
            },
            MenuEntry::Panels(&catalog::APPLICATION),
            MenuEntry::Panels(&catalog::ARRANGEMENT),
            MenuEntry::Panels(&catalog::CONTROLS),
            MenuEntry::Panels(&catalog::CATALOGUE),
            MenuEntry::Panels(&catalog::DETAILS),
            MenuEntry::Panels(&catalog::VISUALIZERS),
            // Drawn only while the flag is on, the gate [`section_shows`]
            // applies for every menu that renders this table. It was
            // missing here entirely, so turning experimental features on
            // grew the right-click Add Panel flyout, which reads the
            // catalog directly, and left this menu one group short.
            MenuEntry::Panels(&catalog::EXPERIMENTAL),
        ],
    },
];

/// Whether a catalog section shows in the menus as things stand: the
/// experimental run stays out until the Development page turns it on, the
/// same gate [`catalog::sections`] applies to the pickers that read the
/// table directly.
pub(crate) fn section_shows(section: &'static PanelSection) -> bool {
    !catalog::is_experimental(section) || settings::experimental()
}

/// The keybinding a dropdown row trails, Zed-style, matching [`init`]'s
/// bindings. Only the primary chord shows; secondaries like Ctrl+I stay
/// off the label.
/// The label and icon a dropdown row shows, read live. Most rows are static,
/// but the Play/Pause toggle flips both to match the player: "Pause" while
/// playing, "Play" while stopped or paused.
pub(crate) fn menu_item_display(item: MenuItem, is_playing: bool) -> (&'static str, &'static str) {
    match item.action {
        MenuAction::TogglePlayback if is_playing => {
            (rox_i18n::t_static("menu-pause"), icons::PAUSE)
        }
        // `item.label` is an i18n key, not display text. See `menu_section`.
        _ => (rox_i18n::t_static(item.label), item.icon),
    }
}

/// Whether a dropdown row trails the signal glyph: a catalog row for a
/// panel whose settings have knobs the shared pool can drive. Read by
/// every menu that renders the catalog, so the mark can't be in one list
/// and missing from the next.
pub(crate) fn signal_marked(action: MenuAction) -> bool {
    matches!(action, MenuAction::OpenPanel(def) if catalog::supports_signals(def))
}

pub(crate) fn shortcut_for(action: MenuAction) -> Option<&'static str> {
    match action {
        MenuAction::TogglePlayback => Some("Space"),
        MenuAction::OpenSettings => Some(if cfg!(target_os = "macos") {
            "Cmd-,"
        } else {
            "Ctrl-,"
        }),
        MenuAction::OpenStats => Some(if cfg!(target_os = "macos") {
            "Cmd-Shift-S"
        } else {
            "Ctrl-Shift-S"
        }),
        MenuAction::TogglePostShader => Some(if cfg!(target_os = "macos") {
            "Cmd-Shift-X"
        } else {
            "Ctrl-Shift-X"
        }),
        _ => None,
    }
}

/// A layout action waiting on its dialog, floated over the window: naming a
/// new preset to save, or confirming an apply that replaces the current
/// arrangement.
enum LayoutDialog {
    Save(Entity<InputState>),
    ConfirmOverwrite(String),
    ConfirmApply(String),
    /// Naming a new workspace bundle built from the current look.
    SaveWorkspace(Entity<InputState>),
    /// Replacing a saved workspace of the same name with the current look.
    ConfirmOverwriteWorkspace(String),
    /// Applying a saved or shipped workspace, which replaces the whole look.
    /// Holds the card the confirm reads out, built when the dialog opens so
    /// the bundle behind it isn't reparsed every frame the dialog is up.
    ConfirmApplyWorkspace {
        card: crate::workspaces::ApplyCard,
        /// Whether the bundle just arrived from a file, which changes what
        /// the dialog says: an import has already saved it, so the offer is
        /// to apply it now rather than to replace what's there.
        imported: bool,
    },
    /// Taking a pinned panel out of the layout. Holds what to close so the
    /// confirm can do it without going looking again.
    ConfirmCloseLocked {
        panel: Arc<dyn PanelView>,
        tabs: WeakEntity<TabPanel>,
        name: SharedString,
    },
}

/// How a workspace window opens, which the menubar's Window entries pick.
pub enum WorkspaceStart {
    /// Launch and plain New Window: restore the saved working layout, and
    /// on launch the last playing track.
    Restore,
    /// A blank dock the user fills from the Panels menu.
    Empty,
    /// Built from a named preset's dump.
    Preset(String),
}

pub struct Workspace {
    open_menu: Option<usize>,
    /// Which submenu entry of the open dropdown is flown out, by entry
    /// index. Hovering an entry moves it, closing the menu clears it.
    open_submenu: Option<usize>,
    /// The second level, for the one flyout that has groups inside it: the
    /// Window menu's panel picker, whose rows are the presets and the catalog
    /// groups. Indexed within that flyout, cleared with the level above it.
    open_subgroup: Option<usize>,
    /// The painted bounds of the open dropdown ([0]) and the panel picker's
    /// flyout ([1]), captured each frame they draw. The next frame's flyouts
    /// read them to pick a side, the same measure-then-decide the
    /// gpui-component menus run on.
    menu_surfaces: [Option<Bounds<Pixels>>; 2],
    /// The window width at the last menu paint, the other half of the
    /// side decision.
    menu_viewport_w: Pixels,
    /// A mouse button is held down somewhere in the window. Alt+drag is
    /// the compositor's window move/resize, so an alt-revealed menubar
    /// stays hidden while a button is down: the overlay must never be in
    /// front of the drag. Tracked in the capture phase so an occluding
    /// child can't hide the press.
    pointer_down: bool,
    /// The Alt press run behind the pin below: two clean taps of the key
    /// leave a hidden menubar up.
    alt_tap: menubar::AltTap,
    /// A hidden menubar is pinned up: it stays without Alt held, so its
    /// buttons can be clicked unmodified. Dropped when the pointer leaves
    /// the bar, on a press below it, on escape, or on another double-tap.
    menubar_pinned: bool,
    /// The pinned bar has had the pointer over it, so the next time the
    /// pointer leaves it the pin drops. Without this the pin would fall the
    /// instant it went up, before the pointer ever reached the bar.
    menubar_touched: bool,
    state: AppState,
    /// Fallback focus so the key bindings keep a dispatch path under the
    /// Workspace context even before a panel takes focus. The dock focuses
    /// the active panel on activation and takes over from there.
    focus: FocusHandle,
    dock: Entity<DockArea>,
    /// The root of the one layout tree: center tabs over the transport
    /// row, vertically. One tree rather than center-plus-bottom-dock so
    /// closing or moving everything in one region collapses the rest up
    /// into the space.
    stack: Entity<StackPanel>,
    /// The tab panel the layout starts with. Panels-menu panels go here
    /// while it's still showing.
    center_tabs: Entity<TabPanel>,
    /// The transport row's stack: the transport groups at start, and the
    /// row Panels-menu audio panels append to.
    bottom_stack: Entity<StackPanel>,
    /// The debounce for layout saves; replacing it cancels the running
    /// timer, so only a quiet layout dumps.
    save_task: Option<Task<()>>,
    /// Why this window's layout fell back to empty, shown by the empty
    /// hint until a layout change leaves panels standing. None when the
    /// empty start was asked for.
    layout_error: Option<SharedString>,
    /// The mini-player button's two presets, by name. Cached off the settings
    /// file so the menubar never reads disk per frame; the settings window
    /// pushes changes back through [`Workspace::set_mini_roles`]. The button
    /// hides unless a mini preset is set.
    primary_layout: Option<String>,
    mini_layout: Option<String>,
    /// The named preset this window is on, copied to settings on every
    /// apply. Which side of the mini toggle shows falls out of comparing it to
    /// `mini_layout`; a workspace save captures into it. None is an unnamed
    /// arrangement.
    active_layout: Option<String>,
    /// The layout save/apply dialog while it's up; dropped on close.
    layout_dialog: Option<LayoutDialog>,
    /// Submits the save dialog's name field on Enter.
    _layout_input: Option<Subscription>,
    /// The quick-play modal while it's up; dropped on dismiss.
    quick_play: Option<Entity<QuickPlay>>,
    /// Clears `quick_play` and hands focus back when the modal dismisses.
    _quick_play_dismissed: Option<Subscription>,
    /// The queue modal the queue widget opens when no queue panel is docked;
    /// a throwaway queue panel floated over the workspace, dropped on close.
    queue_modal: Option<Entity<QueuePanel>>,
    /// This window's slice of the backdrop: what it painted last, for
    /// retiring the texture on a new bake.
    backdrop: WindowBackdrop,
    /// The playing path the window title currently reflects; None while
    /// the title is the plain app name. Compared each player tick so the
    /// tag lookup and the platform title call only run on a track change.
    titled_track: Option<TrackKey>,
    _layout_changed: Subscription,
    /// The player pump notifies every tick while a session runs; the
    /// title refresh hangs off it and bails on the path compare.
    _player_changed: Subscription,
    /// The menubar's right side shows the catalog status, so library
    /// updates must repaint the workspace.
    _library_changed: Subscription,
    /// A recorded listen bumps its track's play count in the shared
    /// projection, so plays columns move without a reload.
    _history_changed: Subscription,
    /// A new bake must repaint the window that shows it.
    _backdrop_changed: Subscription,
    /// Keeps the window registry frontmost-first, so the tray, the native
    /// menu, and the file handoff act on the window the user was last in.
    _window_activated: Subscription,
    /// The OS media service, on the primary window only: the D-Bus name is
    /// per-process, so a second window never registers its own. `None` on
    /// every other window and when the platform backend won't come up. The
    /// window holds it rather than owns it: a close hands it to a surviving
    /// window or to the tray, and it only dies when the app does.
    media: Option<Entity<MediaSession>>,
    /// The whole-window post shader this window runs, None while the
    /// setting is off or pathless. See [`PostShaderDriver`].
    post_shader: Option<PostShaderDriver>,
}

/// A one-group tabs item plus the TabPanel entity inside it, for wiring the
/// group into a stack.
fn tabs_item(
    panels: Vec<Arc<dyn PanelView>>,
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> (DockItem, Entity<TabPanel>) {
    let item = DockItem::tabs(panels, weak_dock, window, cx);
    let view = match &item {
        DockItem::Tabs { view, .. } => view.clone(),
        _ => unreachable!("DockItem::tabs builds a Tabs item"),
    };
    (item, view)
}

/// The split's StackPanel entity, for keeping a handle to a stack a
/// DockItem builder created.
fn split_view(item: &DockItem) -> Entity<StackPanel> {
    match item {
        DockItem::Split { view, .. } => view.clone(),
        _ => unreachable!("split_with_sizes builds a Split item"),
    }
}

/// The starting layout: the library tab group over the transport row.
/// Returns the center item plus the workspace's add targets: the root
/// stack, the center tabs, and the transport row's stack.
/// A blank starting layout: one empty tab group in the root stack, no
/// transport row. The group renders to nothing while empty, so the window
/// is bare until the Panels menu adds something; the detached transport
/// stack comes back with it for [`Workspace::add_bottom`] to attach on
/// first use, same as a restored layout with no row.
fn empty_layout(
    weak_dock: &WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> (
    DockItem,
    Entity<StackPanel>,
    Entity<TabPanel>,
    Entity<StackPanel>,
) {
    let (tabs, center_tabs) = tabs_item(Vec::new(), weak_dock, window, cx);
    let center = DockItem::split_with_sizes(
        Axis::Vertical,
        vec![tabs],
        vec![None],
        weak_dock,
        window,
        cx,
    );
    let stack = split_view(&center);
    let bottom_stack = cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx));
    (center, stack, center_tabs, bottom_stack)
}

/// Pull the workspace's add targets back out of a restored layout: the
/// root stack, the first tab group (where add_center prefers to put
/// things), and the last horizontal split (the transport row add_bottom
/// appends to).
/// The latter two are heuristics over a tree the user may have rearranged,
/// so they can come up empty.
fn layout_views(
    item: &DockItem,
) -> (
    Entity<StackPanel>,
    Option<Entity<TabPanel>>,
    Option<Entity<StackPanel>>,
) {
    let DockItem::Split { view, items, .. } = item else {
        unreachable!("a restored root stack is a Split item");
    };
    let center_tabs = items.iter().find_map(|child| match child {
        DockItem::Tabs { view, .. } => Some(view.clone()),
        _ => None,
    });
    let bottom = items.iter().rev().find_map(|child| match child {
        DockItem::Split { axis, view, .. } if *axis == Axis::Horizontal => Some(view.clone()),
        _ => None,
    });
    (view.clone(), center_tabs, bottom)
}

/// What a window opening over the tray's hold takes back from it: the live
/// shared state, and the OS media service when it stayed up while no window
/// was open. Only a reopen has one; every other open starts from nothing.
pub struct Adopted {
    pub state: AppState,
    pub media: Option<Entity<MediaSession>>,
}

impl Workspace {
    pub fn new(
        start: WorkspaceStart,
        adopt: Option<Adopted>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A reopen from the tray adopts the state the last close handed to
        // the hold, so the playing player, library, and art come straight
        // over; every other open builds its own world.
        let adopted = adopt.is_some();
        let (adopt, adopt_media) = match adopt {
            Some(adopt) => (Some(adopt.state), adopt.media),
            None => (None, None),
        };
        let state = adopt.unwrap_or_else(|| {
            let player = cx.new(Player::new);
            let library = cx.new(Library::new);
            // The catalog says when a scan starts and when a watch sync has
            // settled; what happens next is the app's, not the service's.
            // Both hang off the app rather than this window, so a close to
            // the tray leaves them on the library the hold keeps.
            crate::integrations::taskbar::follow(&library, cx);
            crate::embeddings::follow(&library, cx);
            crate::replaygain_job::follow(&library, cx);
            crate::tempo_job::follow(&library, cx);
            let scrobbler = cx.new(|cx| Scrobbler::new(&player, &library, cx));
            let discord = cx.new(|cx| DiscordPresence::new(&player, &library, cx));
            AppState {
                thumbs: cx.new(|cx| Thumbs::new(&library, cx)),
                portraits: cx.new(|_| Portraits::default()),
                history: cx.new(|cx| History::new(&scrobbler, cx)),
                scrobbler,
                discord,
                library,
                now_art: cx.new(|cx| NowPlayingArt::new(player.clone(), cx)),
                player,
                selection: cx.new(|cx| Selection::new(cx)),
                query: cx.new(|_| SharedQuery::default()),
                tab_hosts: cx.new(|_| TabHosts::default()),
                signals: Arc::new(rox_viz::signal::SignalHub::new(
                    Settings::load().look.bundle.signals,
                )),
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus);

        // The first workspace window is the launch, so it brings back what
        // was playing: the saved id resolves against the library and loads
        // paused at the saved position. A track gone from the library
        // resolves to nothing and the start stays cold. New Window opens
        // idle; its player is its own.
        let settings = Settings::load();
        // The mini-player roles are kept in the struct so the menubar never
        // reads the file per frame; captured before the layout field moves
        // out below.
        let primary_layout = settings.look.bundle.primary_layout.clone();
        let mini_layout = settings.look.bundle.mini_layout.clone();
        // Which named preset this window opens on: the persisted one on a
        // restore, the named preset a preset window built from, and nothing
        // for an empty window. A restore that falls back to the default
        // arrangement below still claims the saved name, which the next apply
        // or save corrects.
        let active_layout = match &start {
            WorkspaceStart::Restore => settings.look.active_layout.clone(),
            WorkspaceStart::Preset(name) => Some(name.clone()),
            WorkspaceStart::Empty => None,
        };
        // The first window to open is the primary: it restores the last track
        // and owns the OS media service. The global is still empty here; this
        // window joins it below, so a later New Window reads false.
        let is_primary = cx.default_global::<WorkspaceWindows>().open.is_empty();
        // An adopted player is already where the user left it, often
        // playing; the launch restore would yank it back to the saved spot.
        if settings.restore_last_track && is_primary && !adopted {
            // Prefer the whole queue: resolve each id back to a path, keeping
            // the explicit flags parallel and realigning the cursor past any
            // entry whose file has left the library. An older file with only
            // last_track falls through to the single-track restore.
            let queue = settings.session.last_queue.as_ref().and_then(|q| {
                let library = state.library.read(cx);
                let mut keys = Vec::with_capacity(q.entries.len());
                let mut explicit = Vec::with_capacity(q.entries.len());
                let mut cursor = 0;
                for (i, entry) in q.entries.iter().enumerate() {
                    let path = library
                        .paths_for(&[entry.id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    if let Some(path) = path {
                        if i <= q.cursor {
                            cursor = keys.len();
                        }
                        // The sub comes off the saved entry rather than the
                        // library: the id names the row, but the projection
                        // that would say which subsong it is may not be
                        // loaded this early in a launch.
                        keys.push(TrackKey {
                            path,
                            sub: entry.sub,
                        });
                        explicit.push(entry.explicit);
                    }
                }
                (!keys.is_empty()).then_some((keys, explicit, cursor, q.position_secs))
            });
            if let Some((keys, explicit, cursor, position_secs)) = queue {
                state.player.update(cx, |player, cx| {
                    player.restore_queue(keys, explicit, cursor, position_secs, cx)
                });
            } else if let Some(last) = settings.session.last_track {
                let path = state
                    .library
                    .read(cx)
                    .paths_for(&[last.id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                if let Some(path) = path {
                    let key = TrackKey {
                        path,
                        sub: last.sub,
                    };
                    state
                        .player
                        .update(cx, |player, cx| player.restore(key, last.position_secs, cx));
                }
            }
        }

        // A first launch (no settings file yet) gets the welcome window
        // over this primary workspace. Deferred through a spawn: this
        // constructor runs inside the window's own open, no place to open
        // another.
        if is_primary && settings::first_run() {
            cx.spawn(async move |this, cx| {
                this.update(cx, |this, cx| {
                    crate::startup::welcome_window::open(this.state.clone(), cx);
                })
                .ok();
            })
            .detach();
        }

        let dock = cx.new(|cx| DockArea::new("rox", Some(LAYOUT_VERSION), window, cx));
        let weak_dock = dock.downgrade();

        register_panels(&state, cx.entity().downgrade(), cx);

        // Where the dock starts, by how the window was opened: launch and
        // plain New Window restore the saved working layout, a preset window
        // builds from that named dump, an empty window skips straight to the
        // blank fallback below. A dump it can't trust (wrong version, no
        // stack root) falls through the same as none.
        let source = match &start {
            WorkspaceStart::Restore => settings.look.layout.clone(),
            WorkspaceStart::Preset(name) => {
                rox_core::settings::layouts::resolve(&settings, name).map(|preset| preset.dump)
            }
            WorkspaceStart::Empty => None,
        };
        // A dump that exists but won't restore, or a preset whose name no
        // longer resolves, is a failure the window should say out loud; a
        // plain start with nothing saved is not.
        let expected = source.is_some() || matches!(start, WorkspaceStart::Preset(_));
        let restored = source
            .and_then(|value| serde_json::from_value::<DockAreaState>(value).ok())
            .filter(|dump| {
                dump.version == Some(LAYOUT_VERSION)
                    && matches!(dump.center.info, PanelInfo::Stack { .. })
            })
            .map(|dump| dump.center.to_item(weak_dock.clone(), window, cx));
        let layout_error = (restored.is_none() && expected).then(|| {
            let message = match &start {
                WorkspaceStart::Preset(_) => {
                    rox_i18n::t!("workspace-layout-preset-restore-failed")
                }
                _ => rox_i18n::t!("workspace-layout-restore-failed"),
            };
            log::warn!("workspace: {message}");
            message
        });

        let (center, stack, center_tabs, bottom_stack) = match restored {
            Some(item) => {
                let (stack, tabs, bottom) = layout_views(&item);
                // The preferred add targets may be gone from a rearranged
                // layout; fresh detached entities take their place, and the
                // add paths attach them back into the tree on first use.
                let tabs = tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
                let bottom = bottom
                    .unwrap_or_else(|| cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx)));
                (item, stack, tabs, bottom)
            }
            // Everything without a restorable dump starts empty: the blank
            // window and the first run because that's what was asked for, a
            // broken dump because an empty window that says why beats a
            // default arrangement nobody arranged.
            None => empty_layout(&weak_dock, window, cx),
        };

        // Save the layout when it settles after a change, and once more on
        // close, which also catches window moves and resizes: those emit no
        // dock events. A change that leaves panels standing also retires the
        // fallback notice: the layout is the user's again.
        let _layout_changed =
            cx.subscribe_in(&dock, window, |this, _, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    if this.layout_error.is_some() && !this.dock_is_empty(cx) {
                        this.layout_error = None;
                    }
                    this.save_layout_soon(window, cx);
                }
            });
        // Observe rather than subscribe: scan progress ticks notify the
        // library without emitting Updated, and the badge needs those too.
        // A catalog change can also retag the playing track, so the title
        // re-derives on the next player tick.
        let _library_changed = cx.observe(&state.library, |this, _, cx| {
            this.titled_track = None;
            cx.notify();
        });
        let _player_changed = cx.observe_in(&state.player, window, |this, _, window, cx| {
            this.refresh_title(window, cx);
            this.publish_tray(cx);
            // The shader's frame loop is in render and sustains itself
            // through frame requests, but only render can start it. Nothing
            // else re-renders the workspace on a resume, so a parked shader
            // would sit frozen until a track change; while one is running,
            // the pump's tick re-arms it.
            if this.post_shader.as_ref().is_some_and(|d| d.active) {
                cx.notify();
            }
            // The native Play/Pause row is baked in, so it has to be rebuilt
            // when the player flips. Read here and handed over, since the
            // rebuild can't reach back through this workspace mid-update.
            let playing = this.state.player.read(cx).is_playing();
            native_menu::sync_playback(playing, cx);
        });
        let _history_changed = cx.subscribe(&state.history, |this, _, event: &HistoryEvent, cx| {
            let HistoryEvent::Recorded { track_id } = *event;
            this.state
                .library
                .update(cx, |library, cx| library.record_play(track_id, cx));
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // Frontmost lookups follow focus, so every workspace window reports
        // its own activation. Deactivation is nobody's business: the window
        // losing focus to a child (settings, a popout) is still the
        // workspace the user is in.
        let _window_activated = cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                note_activated(window.window_handle(), cx);
            }
        });
        let weak_self = cx.entity().downgrade();
        let registry = cx.default_global::<WorkspaceWindows>();
        let opened = registry.next_opened;
        registry.next_opened += 1;
        // At the head: a window opens focused, and the activation observer
        // above only fires on a later focus change.
        registry.open.insert(
            0,
            OpenWorkspace {
                handle: window.window_handle(),
                workspace: weak_self.into(),
                state: state.clone(),
                opened,
            },
        );
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            close_workspace_window(this.upgrade(), window, cx);
            true
        });

        // Zoom needs nothing from the workspace anymore: adding tab panels
        // to stacks makes the dock area subscribe them, and the zoomed
        // panel covers the whole dock area, which is the whole window under
        // the menu bar.
        dock.update(cx, |dock, cx| {
            dock.set_center(center, window, cx);
            dock.set_toggle_button_visible(false, cx);
            // A middle drag released outside the window pops the panel out
            // into its own OS window, same as the menu's Pop Out.
            let state = state.clone();
            dock.on_middle_drag_out(move |panel, _, _, cx| {
                rox_panel_api::panel::pop_out_view(panel, state.clone(), cx);
            });
        });

        // The launch also binds the control socket (ADR 22), which hangs off
        // the app rather than any window: the drain holds its own state
        // clone, so a close to the tray leaves the socket serving. A reopen
        // adopts the same entities the running server is already bound to,
        // so only the first ever open binds. The workspaces disk watch is
        // app-level for the same reason and starts on the same edge.
        if is_primary && !adopted {
            crate::integrations::ipc::serve(&state, cx);
            crate::integrations::broadcast::start(&state, cx);
            crate::workspaces::watch(cx);
        }

        // The primary window owns the OS media service: a session the tray
        // hands back when it kept one alive, a fresh registration otherwise.
        // Everything past that point is the session's own business: it
        // drains the keys onto the player and publishes back out on its own
        // observer, with or without a window in front of it.
        let media = if is_primary {
            adopt_media.or_else(|| MediaSession::new(state.clone(), window, cx))
        } else {
            None
        };

        let mut this = Workspace {
            open_menu: None,
            open_submenu: None,
            open_subgroup: None,
            menu_surfaces: [None; 2],
            menu_viewport_w: Pixels::ZERO,
            pointer_down: false,
            alt_tap: menubar::AltTap::default(),
            menubar_pinned: false,
            menubar_touched: false,
            state,
            focus,
            dock,
            stack,
            center_tabs,
            bottom_stack,
            save_task: None,
            layout_error,
            primary_layout,
            mini_layout,
            active_layout,
            layout_dialog: None,
            _layout_input: None,
            quick_play: None,
            _quick_play_dismissed: None,
            queue_modal: None,
            backdrop: WindowBackdrop::default(),
            titled_track: None,
            _layout_changed,
            _player_changed,
            _library_changed,
            _history_changed,
            _backdrop_changed,
            _window_activated,
            media,
            post_shader: None,
        };
        // Panel surface shaders paint far from any state handle, so the
        // window's hub and player go on the registry they look up.
        panel::shader::note_window(window, &this.state, cx);
        // The configured screen shader goes on as the window opens, so a
        // restart comes up running it without a trip through the settings
        // window.
        this.apply_post_shader(window, cx);
        // With the all-windows option on, the per-window apply above isn't
        // enough: the app-level pass has to run once to cache the source
        // for the child sweeps, or children opened later stay bare.
        let config = Settings::load().post_shader;
        if config.enabled && config.all_windows {
            apply_post_shader(cx);
        }
        this
    }

    /// Install or clear this window's post shader from the settings file.
    /// The resolve and compile happen here, synchronously: a failure logs,
    /// puts its message in the shared readout, and leaves whatever compiled
    /// last still running, so a broken edit never blanks the effect.
    fn apply_post_shader(&mut self, window: &mut Window, cx: &App) {
        let config = Settings::load().post_shader;
        // Keep the live switch and the slot feeds in step; the startup path
        // comes through here before any app-level apply has run.
        POST_SHADER_ON.store(config.enabled, Ordering::Relaxed);
        set_post_shader_routes(config.routes.clone());
        set_post_shader_manual(config.manual.clone());
        if !config.enabled {
            self.clear_post_shader(window);
            // The switch being off doesn't unname the slots. Those names come
            // off the source the config points at rather than off anything
            // that's running, and the Shader page goes on showing the slots
            // as hand-set knobs while the pass is parked. So without this,
            // flipping the shader off turned every row from "bend" back into
            // "slot 0". A config pointing nowhere still leaves them bare,
            // which is the [`clear_post_shader`] case above.
            *POST_SHADER_LABELS.write().unwrap() = post_shader_source(&config)
                .ok()
                .flatten()
                .map(|source| panel::shader::slot_labels(&source))
                .unwrap_or_default();
            return;
        }
        let mut driver = PostShaderDriver {
            path: post_shader_watch(&config),
            stamp: None,
            checked: Instant::now(),
            active: self
                .post_shader
                .as_ref()
                .is_some_and(|driver| driver.active),
            was_live: true,
            meta: [0.0; 8],
            run_when_idle: config.run_when_idle,
            uses_cover: false,
            reads_cursor: false,
            cover: 0,
        };
        driver.stamp = driver.path.as_deref().and_then(settings::file_stamp);
        let (source, ctx) = match post_shader_program(&config) {
            Ok(Some(program)) => program,
            // A pool name that matches no entry, or a config pointing
            // nowhere. Same teardown as the switch being off: there's
            // nothing to run, and leaving the last one up would be running
            // something this look never asked for.
            Ok(None) => {
                self.clear_post_shader(window);
                return;
            }
            // The stamp still moves on, so fixing the file reloads it.
            Err(error) => {
                log::warn!("post shader: {error}");
                *POST_SHADER_ERROR.write().unwrap() = Some(error);
                self.post_shader = Some(driver);
                return;
            }
        };
        // The slot names travel with the source, so the settings window's
        // route editor names them the way a panel's does.
        *POST_SHADER_LABELS.write().unwrap() = panel::shader::slot_labels(&source);
        POST_SHADER_COVERAGE.store(
            if panel::shader::overlay(&source) {
                2
            } else {
                1
            },
            Ordering::Relaxed,
        );
        // A program binding `@cover` registers with the art the feed holds
        // right now, and the frame loop measures track changes against the
        // revision it took.
        driver.uses_cover = panel::shader::uses_cover(&source);
        if driver.uses_cover {
            driver.cover = panel::shader::poll_cover(window, cx);
        }
        driver.reads_cursor = panel::shader::reads_cursor(&source);
        // The whole program: the text splits into its passes here and its
        // images are read from wherever the source resolved from, so a
        // split or an unreadable image ends up in the same readout a naga
        // error does.
        match panel::shader::register_program(window, &source, &ctx) {
            Ok(id) => {
                window.set_post_shader(Some(id));
                driver.active = true;
                *POST_SHADER_ERROR.write().unwrap() = None;
            }
            Err(error) => {
                log::warn!("post shader: {error}");
                *POST_SHADER_ERROR.write().unwrap() = Some(error);
            }
        }
        self.post_shader = Some(driver);
    }

    /// Take the shader off this window and clear the shared readouts: the
    /// state before anything is configured, which is also where a switched
    /// off or unresolvable one ends up.
    fn clear_post_shader(&mut self, window: &mut Window) {
        if self.post_shader.take().is_some_and(|driver| driver.active) {
            window.set_post_shader(None);
        }
        *POST_SHADER_ERROR.write().unwrap() = None;
        *POST_SHADER_LABELS.write().unwrap() = Vec::new();
        POST_SHADER_COVERAGE.store(0, Ordering::Relaxed);
    }

    /// The screen shader's frame loop, run from render: re-read the file
    /// when its stamp moves (one stat a second, the pump cadence render
    /// already runs at), and while audio moves, push the pool's signals into
    /// the shader's slots (slot i takes pool signal i) and ask for the next
    /// frame. A silent hub stops the feed, which freezes the shader clock
    /// and parks the frame requests, the same self-parking discipline the
    /// visualizer panels keep.
    fn drive_post_shader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(driver) = &mut self.post_shader else {
            return;
        };
        // The oldest open workspace handles the app-wide concerns: the
        // child sweeps and their signal feed run once, not per workspace.
        // By open serial, not the list's head: the registry reorders on
        // activation, and the app-wide role shouldn't hop windows every
        // time focus moves.
        let primary = cx
            .default_global::<WorkspaceWindows>()
            .open
            .iter()
            .min_by_key(|w| w.opened)
            .is_some_and(|w| w.handle == window.window_handle());
        if driver.checked.elapsed().as_secs_f32() > 1.0 {
            driver.checked = Instant::now();
            // Only a shader read from a file can change without an apply;
            // the pool and the inline copies come round through one.
            let moved = driver
                .path
                .as_deref()
                .is_some_and(|path| settings::file_stamp(path) != driver.stamp);
            if moved {
                // Through the app-level apply, so every shaded window
                // follows the edit rather than just this one. Never
                // prompts: the hot reload is the authoring loop.
                apply_post_shader(cx);
            } else if primary {
                // Catch child windows opened since the last apply.
                cx.defer(sweep_shaded_children);
            }
        }
        // A program using the track's art follows the track. The poll
        // costs a map read and a path compare per frame until the playing
        // file turns over; then this window re-applies with the new art,
        // and the primary re-shades the children running the same program.
        if self
            .post_shader
            .as_ref()
            .is_some_and(|driver| driver.uses_cover)
        {
            let rev = panel::shader::poll_cover(window, cx);
            if self
                .post_shader
                .as_ref()
                .is_some_and(|driver| driver.cover != rev)
            {
                self.apply_post_shader(window, cx);
                if primary {
                    cx.defer(|cx| {
                        // The children hold ids registered with the old
                        // art; forgotten, the sweep re-registers them and
                        // the new registration replaces the old in place.
                        cx.default_global::<ShadedChildren>().windows.clear();
                        sweep_shaded_children(cx);
                    });
                }
            }
        }
        if !self
            .post_shader
            .as_ref()
            .is_some_and(|driver| driver.active)
        {
            return;
        }
        let run_when_idle = self
            .post_shader
            .as_ref()
            .is_some_and(|driver| driver.run_when_idle);
        let hub = self.state.signals.clone();
        // Tick before the live check. Live only goes true once a tick has
        // seen the feed move, and on a workspace without a particles panel
        // or the signals window this loop is the hub's only ticker, so
        // checking first parked the shader for good: no signals, frozen
        // clock. The tick itself is cheap and TICK_MIN-deduped.
        {
            let player = self.state.player.read(cx);
            hub.tick(&player.feed(), player.playing_entry());
        }
        let live = hub.live();
        // The release tail. Live goes false the moment the audio stops, but
        // a smoothed signal is still falling for a second or two after
        // that, and a shader driven by one needs those frames: without them
        // the last push is the one that sticks and the effect freezes
        // wherever it was rather than fading out.
        let settling = !live && hub.settling();
        let was_live = std::mem::replace(
            &mut self.post_shader.as_mut().expect("checked above").was_live,
            live,
        );
        let signals = post_shader_signals(&hub);
        let meta = panel::shader::meta_slots(window, cx);
        // A pass that reads the pointer keeps its frames coming while the
        // pointer counts for anything, so a paused window still plays out
        // the fade rather than leaving the light stuck wherever the cursor
        // was last seen. It parks itself: presence reaches zero a couple of
        // seconds after the hand stops, and the probe in `render` wakes the
        // window when the hand comes back.
        let cursor_live = self
            .post_shader
            .as_ref()
            .is_some_and(|driver| driver.reads_cursor)
            && meta[6] > 0.0;
        // Meta isn't audio. The theme can flip, the art tint can ease, the
        // volume can move, all while the hub is parked, and a shader
        // tuning itself to the palette has to hear about it or it keeps
        // the theme it was last pushed under until the music moves again.
        // So a changed meta counts as something to say, same as a live
        // hub. It settles by itself: the push stores what it sent, and the
        // next idle frame compares equal and goes back to sleep.
        let stale_meta = self
            .post_shader
            .as_ref()
            .is_some_and(|driver| driver.meta != meta);
        // A hub gone quiet still owes the shader one last word: without it
        // the uniforms freeze mid-song, and a paused window keeps showing
        // the play state on every repaint. The push after the last live
        // frame sends the parked signals and a meta that says stopped.
        // After that the pass sleeps until the music moves again, unless
        // the idle switch keeps its frames coming: the uniforms hold
        // still, and the frames are for the state that updates per draw,
        // which is the mouse.
        if !live && !was_live && !settling && !stale_meta {
            if run_when_idle || cursor_live {
                // The uniforms hold still, but the mouse is stored beside
                // them rather than read per draw, so an idle frame has to
                // push it across itself or the lamp freezes mid-pause.
                window.set_post_mouse();
                window.request_animation_frame();
                if primary {
                    cx.defer(push_child_mouse);
                }
            }
            return;
        }
        if let Some(driver) = self.post_shader.as_mut() {
            driver.meta = meta;
        }
        window.set_post_signals(signals, meta);
        if live || settling || run_when_idle || cursor_live {
            window.request_animation_frame();
        }
        if primary {
            // Deferred: a window can't update its siblings from inside its
            // own render.
            cx.defer(move |cx| push_child_signals(signals, meta, cx));
        }
    }

    /// The dock area, for the settings window's Layout page: the tree
    /// view walks it and export dumps it.
    pub fn dock(&self) -> &Entity<DockArea> {
        &self.dock
    }

    /// Swap in an imported layout dump: the launch restore's checks and
    /// rebuild, on a live workspace. A dump from another version or with
    /// a non-stack root is refused, same as a stale saved layout.
    pub fn apply_layout(
        &mut self,
        dump: DockAreaState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if dump.version != Some(LAYOUT_VERSION)
            || !matches!(dump.center.info, PanelInfo::Stack { .. })
        {
            return false;
        }
        // Fold the outgoing layout's live dock into its working copy before
        // the swap, so switching away keeps its unsaved tweaks. Synchronous,
        // because the debounced save below would otherwise be the only
        // writer, and it dumps the incoming layout, not this one.
        self.stash_active_edits(window, cx);
        // The registry's builders capture one workspace's entities;
        // re-register so the rebuild runs against this one even after
        // another window registered over it.
        register_panels(&self.state, cx.entity().downgrade(), cx);
        let weak_dock = self.dock.downgrade();
        let item = dump.center.to_item(weak_dock.clone(), window, cx);
        let (stack, tabs, bottom) = layout_views(&item);
        self.stack = stack;
        self.center_tabs = tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
        self.bottom_stack =
            bottom.unwrap_or_else(|| cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx)));
        self.dock
            .update(cx, |dock, cx| dock.set_center(item, window, cx));
        self.save_layout_soon(window, cx);
        true
    }

    /// Fall a live workspace back to the blank layout: the reset a workspace
    /// bundle with no resolvable layout applies. Same swap as
    /// [`apply_layout`], but the center is the empty start and the empty
    /// hint says why it's empty, rather than a stand-in arrangement nobody
    /// arranged.
    fn apply_empty_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the outgoing layout's tweaks the same as any other swap.
        self.stash_active_edits(window, cx);
        let weak_dock = self.dock.downgrade();
        let (center, stack, center_tabs, bottom_stack) = empty_layout(&weak_dock, window, cx);
        self.stack = stack;
        self.center_tabs = center_tabs;
        self.bottom_stack = bottom_stack;
        self.dock
            .update(cx, |dock, cx| dock.set_center(center, window, cx));
        self.layout_error = Some(rox_i18n::t!("workspace-workspace-restore-failed"));
        log::warn!("workspace: applied workspace has no restorable layout, starting empty");
        // The empty build has no name of its own.
        self.set_active_layout(None);
        self.save_layout_soon(window, cx);
    }

    /// Record the named preset the window is now on and copy it to settings,
    /// straight away rather than through the debounced layout save, so a
    /// workspace save right after an apply captures the right layout.
    fn set_active_layout(&mut self, name: Option<String>) {
        self.active_layout = name.clone();
        Settings::update(move |s| {
            // The layout in front of you keeps its live dock in
            // `settings.look.layout`, not the working-copy store, so clear any
            // stale copy as it becomes active.
            if let Some(name) = &name {
                s.look.layout_edits.remove(name.as_str());
            }
            s.look.active_layout = name;
        });
    }

    /// The dock's current layout as a JSON value, denoised: the shape saved
    /// into settings and written out in an exported workspace bundle. The
    /// denoise pass matters because [`serde_json::to_value`] widens the dump's
    /// f32 sizes and panel configs to f64, so a raw dump comes out with
    /// 17-digit float tails; [`denoise_f32`] snaps them back to clean numbers.
    fn dock_dump(&self, cx: &App) -> serde_json::Result<serde_json::Value> {
        let mut value = serde_json::to_value(self.dock.read(cx).dump(cx))?;
        denoise_f32(&mut value);
        Ok(value)
    }

    /// Fold this window's live dock into the active layout's working copy,
    /// the unsaved-tweaks store a later switch reads back. A window on an
    /// unnamed arrangement (the default build, a one-off import) has no name
    /// to key on, so this no-ops; its live dock is stored in
    /// `settings.look.layout` for the launch restore instead.
    fn stash_active_edits(&self, window: &Window, cx: &mut Context<Self>) {
        let Some(name) = self.active_layout.clone() else {
            return;
        };
        let Ok(dump) = self.dock_dump(cx) else {
            return;
        };
        // The current window size goes with it, read live off the window
        // rather than the debounced `settings.windows.main`, so a resize made
        // just before the switch comes back with the layout.
        let size = Some(window_size(window));
        Settings::update(move |s| {
            s.look.layout_edits.insert(name, LayoutEdit { dump, size });
        });
    }

    /// Apply a saved preset by name. Returns false when
    /// no preset has that name or its dump is one [`apply_layout`]
    /// refuses.
    pub fn apply_named_layout(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let settings = Settings::load();
        let Some(preset) = rox_core::settings::layouts::resolve(&settings, name) else {
            return false;
        };
        // Size to the preset by default; a working copy with its own size
        // overrides that below, so a resize made while editing comes back too.
        let mut size = preset.size;
        // Prefer the layout's working copy, the unsaved tweaks kept from the
        // last time it was in front of you, over the pristine preset. A
        // missing copy, or one an older version can no longer load, falls
        // back to the saved dump.
        let edited = settings.look.layout_edits.get(name).cloned();
        let mut applied = false;
        if let Some(edit) = &edited {
            if let Ok(dump) = serde_json::from_value::<DockAreaState>(edit.dump.clone()) {
                applied = self.apply_layout(dump, window, cx);
                // The working copy's own size wins when it has one; a copy
                // from before sizes were stored keeps the preset's.
                if applied && edit.size.is_some() {
                    size = edit.size;
                }
            }
        }
        if !applied {
            let Ok(dump) = serde_json::from_value::<DockAreaState>(preset.dump) else {
                return false;
            };
            applied = self.apply_layout(dump, window, cx);
        }
        if !applied {
            return false;
        }
        self.set_active_layout(Some(name.to_string()));
        // Size the window to whichever source won above (the working copy's
        // size, or the preset's); neither having one leaves the window as is.
        // Except in a macOS fullscreen Space, where AppKit owns the frame and
        // a resize is dropped or fights the Space: swap the layout, leave the
        // frame alone. The mini toggle exits fullscreen before applying, so
        // its resize still takes. A `--window-size` launch also pins the
        // frame: preset sizes stay stored, they just don't move the window
        // that session, so every look screenshots at the flag's one size.
        if let Some(size) = size {
            if !(cfg!(target_os = "macos") && window.is_fullscreen())
                && crate::window_size_override().is_none()
            {
                resize_clamped(window, size);
            }
        }
        // A programmatic resize only shows on the next drawn frame, and gpui
        // stops pumping frames for a window that's idle and not focused.
        // Applying from the settings window leaves this one in exactly that
        // state, so the resized dock sat stale until the compositor woke it
        // on the next focus, which is why it looked gone until you tabbed
        // back. Wake it and mark it dirty so the new layout draws now.
        window.activate_window();
        window.refresh();
        true
    }

    /// Apply a shipped or saved workspace to this window: the whole look
    /// through the shared path, then this window's dock swaps to the bundle's
    /// primary layout, or resets to the default arrangement when the bundle
    /// has no layout. The empty launcher's way to start from a vendored
    /// look; a blank window has nothing to replace, so it acts straight off
    /// the click with no confirm. The settings window's apply comes through
    /// here too, so both entry points share one flow.
    pub(crate) fn apply_workspace(
        &mut self,
        name: &str,
        shaders: ApplyShaders,
        notice: ShaderNotice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        // Without Shaders takes them out here, before anything reads the
        // bundle: the overlay and every panel with one on it, the pool left
        // alone. Everything downstream then applies a look that doesn't
        // have any, so there's no second reading of the choice.
        let bundle = match shaders {
            ApplyShaders::Wear => bundle,
            ApplyShaders::Skip => crate::workspaces::without_shaders(&bundle),
        };
        // What the screen shader is running right now, read before anything
        // moves: it's both the revert target for the countdown below and the
        // thing the incoming one is compared against. Resolved against the
        // pool that's still live, since that's what's actually on screen.
        let prior = Settings::load().post_shader;
        let prior_source = prior
            .enabled
            .then(|| post_shader_source(&prior).ok().flatten())
            .flatten();
        crate::workspaces::apply_look(&bundle, cx);
        // The look's signal pool replaces the live one the same wholesale
        // way; apply_look already persisted it into settings.
        self.state.signals.set_pool(bundle.signals.clone());
        // The shader pool goes over the live one exactly the same way, and
        // apply_look persisted it in the same write. A saved bundle arrives
        // with its file bookmarks scrubbed, so anything still in the
        // shaders folder gets linked back up first; that's what keeps hot
        // reload alive across a save and reapply, and it's the one part
        // worth a second write.
        let mut pool = bundle.shaders.clone();
        if crate::workspaces::relink_ejected(&bundle.name, &mut pool) {
            settings::set_shader_pool(pool);
        } else {
            settings::note_shader_pool(pool);
        }
        // The screen shader belongs to the look too, but it's stored in the
        // machine settings rather than in the bundle apply_look wrote, so it
        // goes in here. A bundle that brings none applies as the disabled
        // default: an apply replaces the look wholesale, and leaving the old
        // shader running over a new look isn't that.
        let incoming = bundle.post_shader.clone().unwrap_or_default();
        let persist = incoming.clone();
        Settings::update(move |s| s.post_shader = persist);
        apply_post_shader(cx);
        // The backdrop shader is inside the bundle apply_look already
        // wrote, so only the cache needs the news.
        settings::note_backdrop_shader(bundle.backdrop_shader.clone());
        // A shader that came in with the look gets the keep-or-revert window
        // a risky apply from the settings page does, but only where nothing
        // said it was coming: the confirms name the shader and the hotkey
        // that turns it off, and asking again the moment it's applied is the
        // second warning for one decision. Either way it's only a change
        // worth proving: an apply that turns the shader off, or installs the
        // same source that was already running, prompts nothing.
        let landed = incoming
            .enabled
            .then(|| post_shader_source(&incoming).ok().flatten())
            .flatten();
        if notice == ShaderNotice::Ask && landed.is_some() && landed != prior_source {
            let player = self.state.player.entity_id();
            crate::settings::shader_confirm::open(
                prior,
                player,
                self.state.now_art.clone(),
                |_| {},
                cx,
            );
        }
        // A whole-look swap drops the previous layout's unsaved edits along
        // with the rest of the old look (apply_look cleared the store); forget
        // the old active name too, so the apply below doesn't stash a stale
        // copy back into the freshly cleared store.
        self.active_layout = None;
        // The mini roles are cached off the file for the menubar; apply_look
        // already persisted them, this just moves the live copy.
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        // The bundle's primary layout fills the window. A bundle without one
        // (or whose named layout no longer resolves) resets to the empty
        // layout with the fallback notice rather than leaving the previous
        // workspace's dock in place, since applying a workspace replaces the
        // look wholesale.
        let applied = bundle
            .primary_layout
            .clone()
            .is_some_and(|primary| self.apply_named_layout(&primary, window, cx));
        if !applied {
            self.apply_empty_layout(window, cx);
        }
        cx.notify();
    }

    /// The mini-player button's config, so the settings window can push a
    /// role change back and the menubar picks it up without a file read.
    pub fn set_mini_roles(&mut self, primary: Option<String>, mini: Option<String>) {
        self.primary_layout = primary;
        self.mini_layout = mini;
    }

    /// Whether a mini layout is assigned, the gate every mini toggle
    /// shows behind.
    pub(crate) fn mini_assigned(&self) -> bool {
        self.mini_layout.is_some()
    }

    /// Whether the window is on the mini preset, the side of the toggle that
    /// decides the glyph and which way the next click goes. Falls out of the
    /// active layout rather than a separate flag, so it's always in step with
    /// what's actually showing.
    pub(crate) fn on_mini(&self) -> bool {
        self.mini_layout.is_some() && self.active_layout == self.mini_layout
    }

    /// A presets-flyout pick, shared with the menu panel: build the saved
    /// panel and either put it in this window, where its kind says panels of
    /// that sort go, or open it in a window of its own. A preset deleted
    /// while the menu stood open does nothing.
    pub(crate) fn run_panel_preset(
        &mut self,
        name: String,
        target: PanelTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(preset) = rox_core::settings::panel_presets::resolve(&Settings::load(), &name)
        else {
            return;
        };
        let dock = self.dock.downgrade();
        let Some(panel) = panel_presets::build(&preset, dock, window, cx) else {
            return;
        };
        match target {
            PanelTarget::Open => match panel_presets::placement_for(&preset) {
                PanelPlacement::Center => self.add_center(panel, window, cx),
                PanelPlacement::Bottom => self.add_bottom(panel, window, cx),
                PanelPlacement::Top => self.add_top(panel, window, cx),
            },
            PanelTarget::NewWindow => panel::open_panel_window(panel, self.state.clone(), cx),
        }
    }

    /// A catalog pick from the Window menu's panel flyout: the panel with its
    /// stock config, in a window of its own.
    pub(crate) fn open_panel_window(
        &mut self,
        def: &'static PanelDef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = (def.build)(&self.state, cx.entity().downgrade(), window, cx);
        panel::open_panel_window(panel, self.state.clone(), cx);
    }

    /// A layouts-flyout pick, shared with the menu panel: open a preset
    /// window, or stage the overwrite/apply confirm dialog.
    pub(crate) fn run_layout(
        &mut self,
        name: String,
        target: LayoutTarget,
        cx: &mut Context<Self>,
    ) {
        match target {
            LayoutTarget::NewWindow => crate::open_workspace_with(WorkspaceStart::Preset(name), cx),
            LayoutTarget::Overwrite => {
                self.layout_dialog = Some(LayoutDialog::ConfirmOverwrite(name));
                cx.notify();
            }
            LayoutTarget::Apply => {
                self.layout_dialog = Some(LayoutDialog::ConfirmApply(name));
                cx.notify();
            }
        }
    }

    /// A workspaces-flyout pick, shared with the menu panel: stage the
    /// overwrite or apply confirm, since either replaces a whole look
    /// wholesale.
    pub(crate) fn run_workspace(
        &mut self,
        name: String,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) {
        self.layout_dialog = Some(match target {
            WorkspaceTarget::Overwrite => LayoutDialog::ConfirmOverwriteWorkspace(name),
            WorkspaceTarget::Apply => LayoutDialog::ConfirmApplyWorkspace {
                card: crate::workspaces::ApplyCard::for_name(&name),
                imported: false,
            },
        });
        cx.notify();
    }

    /// Open the save-workspace dialog: a focused name field that Enter or the
    /// button commits into a bundle of the current look.
    pub(crate) fn open_save_workspace_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("workspace-dialog-workspace-name-placeholder"))
        });
        self._layout_input =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit_save_workspace(window, cx);
                    }
                }),
            );
        window.focus(&input.focus_handle(cx));
        self.layout_dialog = Some(LayoutDialog::SaveWorkspace(input));
        cx.notify();
    }

    /// Save the current look under the dialog's name as a new workspace. An
    /// empty name waits; a name already saved routes through the overwrite
    /// confirm, matching the settings window's Save Current. A shipped
    /// bundle's name saves straight through with no confirm: the new user
    /// bundle shadows it wherever names resolve, which is the expected way
    /// to fork a shipped look.
    fn commit_save_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LayoutDialog::SaveWorkspace(input)) = &self.layout_dialog else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        // Flush the live dock first. Panel config like the library's column
        // arrangement is only written to the settings file on the next layout
        // dump, so without this the bundle would capture whatever's stale on
        // disk.
        self.persist(window, cx);
        if crate::workspaces::path_for(&name).exists() {
            self.layout_dialog = Some(LayoutDialog::ConfirmOverwriteWorkspace(name));
            self._layout_input = None;
            cx.notify();
            return;
        }
        crate::workspaces::store(&crate::workspaces::snapshot(&name, &Settings::load()));
        self.close_layout_dialog(window, cx);
    }

    /// Replace the pending workspace with the current look, the confirm
    /// dialog's yes. Only user bundles get this confirm, and one deleted
    /// since the dialog opened just comes back: the write goes to the file
    /// the name picks either way.
    fn overwrite_workspace_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmOverwriteWorkspace(name)) => name.clone(),
            _ => return,
        };
        // Flush the live dock so the overwrite captures current panel config,
        // not the stale disk copy. See commit_save_workspace.
        self.persist(window, cx);
        // The bundle's name picks its file, so an overwrite writes back over
        // the same one a first save wrote: both are the one write, and the
        // card the old file held comes along through the snapshot.
        crate::workspaces::store(&crate::workspaces::snapshot(&name, &Settings::load()));
        self.close_layout_dialog(window, cx);
    }

    /// Apply the pending workspace to this window, the confirm dialog's yes.
    /// `shaders` is the difference between the dialog's two yes buttons: the
    /// running one agrees to whatever code the bundle brought, which is the
    /// one click on this whole path that may write the approved list, and the
    /// bare one strips the shaders out of the look on the way in.
    fn apply_workspace_confirmed(
        &mut self,
        shaders: ApplyShaders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmApplyWorkspace { card, .. }) => {
                if shaders == ApplyShaders::Wear {
                    card.approve_shaders();
                }
                card.name.clone()
            }
            _ => return,
        };
        self.apply_workspace(&name, shaders, ShaderNotice::Told, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Pick a workspace file and add it to the collection, the settings
    /// window's Import path from the menu.
    ///
    /// A bundle with shaders this machine has never agreed to run opens
    /// the apply confirm on the way in, so what arrived is read out as it
    /// arrives rather than a week later when somebody applies it.
    /// Backing out of that dialog is exactly the old behaviour: the file is
    /// saved, nothing is approved, and nothing is running it.
    fn import_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
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
            let Some(bundle) = crate::workspaces::read_bundle(&path) else {
                return;
            };
            crate::workspaces::store(&bundle);
            let card = crate::workspaces::ApplyCard::of(&bundle);
            this.update(cx, |this, cx| {
                if !card.shaders.is_empty() {
                    this.layout_dialog = Some(LayoutDialog::ConfirmApplyWorkspace {
                        card,
                        imported: true,
                    });
                }
                cx.notify();
                native_menu::rebuild(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Toggle the mini layout: on the mini preset the click goes back to the
    /// primary, on anything else it goes to the mini preset. The named preset
    /// is the whole story now, so there's no stash to restore and no separate
    /// flag to flip; whichever side we end on becomes the active layout, and
    /// the glyph follows. A missing target (no primary to return to, no mini
    /// to enter) leaves the dock where it is.
    pub(crate) fn toggle_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = if self.on_mini() {
            self.primary_layout.clone()
        } else {
            self.mini_layout.clone()
        };
        let Some(name) = target else {
            return;
        };
        // On macOS the window may be in its own fullscreen Space, where
        // AppKit owns the frame: the layout's resize is dropped and the mini
        // player would come up stretched over the whole screen. Leave
        // fullscreen first and apply the layout once the exit transition has
        // finished, so the resize hits a normal window.
        #[cfg(target_os = "macos")]
        if window.is_fullscreen() {
            window.toggle_fullscreen();
            let name = name.clone();
            cx.spawn_in(window, async move |this, cx| {
                // gpui exposes no did-exit hook, so poll: the style mask
                // cleared and the frame stable across two ticks means the
                // animation is done. The cap keeps a stuck transition from
                // swallowing the toggle.
                let mut last = None;
                for _ in 0..40 {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    let Ok(state) = this
                        .update_in(cx, |_, window, _| (window.is_fullscreen(), window.bounds()))
                    else {
                        return;
                    };
                    match state {
                        (false, bounds) if last == Some(bounds) => break,
                        (_, bounds) => last = Some(bounds),
                    }
                }
                this.update_in(cx, |this, window, cx| {
                    if this.apply_named_layout(&name, window, cx) {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        if self.apply_named_layout(&name, window, cx) {
            cx.notify();
        }
    }

    /// Open the save dialog: a focused name field that Enter or the button
    /// commits into a new preset.
    pub(crate) fn open_save_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("workspace-dialog-layout-name-placeholder"))
        });
        self._layout_input =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit_save(window, cx);
                    }
                }),
            );
        window.focus(&input.focus_handle(cx));
        self.layout_dialog = Some(LayoutDialog::Save(input));
        cx.notify();
    }

    /// Save the current arrangement under the dialog's name, a new preset or
    /// an update to one that already has that name. An empty name waits.
    fn commit_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LayoutDialog::Save(input)) = &self.layout_dialog else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let Ok(dump) = self.dock_dump(cx) else {
            return;
        };
        let size = Some(window_size(window));
        Settings::update(move |s| {
            // Committing the edits clears the working copy; the saved preset
            // is the state now.
            s.look.layout_edits.remove(name.as_str());
            if let Some(existing) = s.look.bundle.layouts.iter_mut().find(|l| l.name == name) {
                existing.dump = dump;
                existing.size = size;
            } else {
                s.look.bundle.layouts.push(NamedLayout { name, dump, size });
            }
        });
        self.close_layout_dialog(window, cx);
    }

    /// Replace a preset with the current arrangement and window size. The
    /// push fallback covers a preset deleted since the menu listed it.
    fn overwrite_layout(&mut self, name: &str, window: &Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        let Ok(dump) = self.dock_dump(cx) else {
            return;
        };
        let size = Some(window_size(window));
        Settings::update(move |s| {
            // Overwriting is a save under the pending name; the working copy
            // it replaces is now the saved preset.
            s.look.layout_edits.remove(name.as_str());
            if let Some(existing) = s.look.bundle.layouts.iter_mut().find(|l| l.name == name) {
                existing.dump = dump;
                existing.size = size;
            } else {
                s.look.bundle.layouts.push(NamedLayout { name, dump, size });
            }
        });
        cx.notify();
    }

    /// Overwrite the pending preset with the current arrangement, the
    /// confirm dialog's yes.
    fn overwrite_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmOverwrite(name)) => name.clone(),
            _ => return,
        };
        self.overwrite_layout(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Apply the pending preset to this window, the confirm dialog's yes.
    fn apply_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmApply(name)) => name.clone(),
            _ => return,
        };
        self.apply_named_layout(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Ask before taking a pinned panel out of the layout, the Close entry's
    /// route when the panel is locked. The pin is there to shrug off stray
    /// clicks, so the entry asks rather than swallowing the click: a menu
    /// item that does nothing reads as broken.
    pub(crate) fn confirm_close_locked(
        &mut self,
        panel: Arc<dyn PanelView>,
        tabs: WeakEntity<TabPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = panel
            .tab_name(cx)
            .unwrap_or_else(|| panel::display_name(panel.panel_name(cx)).into());
        self.layout_dialog = Some(LayoutDialog::ConfirmCloseLocked { panel, tabs, name });
        window.focus(&self.focus);
        cx.notify();
    }

    /// Close the pinned panel the confirm named. The lock stays what it is;
    /// this one click gets through it.
    fn close_locked_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(LayoutDialog::ConfirmCloseLocked { panel, tabs, .. }) = &self.layout_dialog else {
            return;
        };
        let panel = panel.clone();
        if let Some(tabs) = tabs.upgrade() {
            tabs.update(cx, |tabs, cx| tabs.remove_panel(panel, window, cx));
        }
        self.close_layout_dialog(window, cx);
    }

    /// Drop the layout dialog and hand focus back to the workspace so the
    /// playback keys keep working.
    fn close_layout_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layout_dialog = None;
        self._layout_input = None;
        window.focus(&self.focus);
        cx.notify();
        // Every save, overwrite, and apply comes through here on its way out,
        // so this is the one place that catches a workspace or layout the
        // native menu's baked-in submenus would otherwise miss. A cancel rebuilds
        // an identical bar, which costs nothing worth branching for.
        native_menu::rebuild(cx);
    }

    /// Open the quick-play modal, or close it when it's already up. The
    /// modal takes the keyboard through its search input; dismissal hands
    /// focus back to the workspace so the playback keys keep working.
    fn toggle_quick_play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_play.take().is_some() {
            self._quick_play_dismissed = None;
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        let modal = cx.new(|cx| QuickPlay::new(self.state.clone(), window, cx));
        self._quick_play_dismissed =
            Some(
                cx.subscribe_in(&modal, window, |this, _, _: &DismissEvent, window, cx| {
                    this.quick_play = None;
                    this._quick_play_dismissed = None;
                    window.focus(&this.focus);
                    cx.notify();
                }),
            );
        window.focus(&modal.read(cx).focus_handle(cx));
        self.quick_play = Some(modal);
        cx.notify();
    }

    /// Open the queue modal, or close it when it's already up. The queue
    /// widget calls this when no queue panel is docked, so a click always
    /// does something. A fresh queue panel each open, dropped on close; its
    /// view (columns, headings) is stored in settings, so it comes back the
    /// way it was left.
    pub(crate) fn toggle_queue_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_modal.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        let modal = cx.new(|cx| QueuePanel::windowed(self.state.clone(), window, cx));
        window.focus(&modal.read(cx).focus_handle(cx));
        self.queue_modal = Some(modal);
        cx.notify();
    }

    /// Drop the queue modal and hand focus back to the workspace, so the
    /// playback keys keep working. The scrim's click-out and the card's
    /// Escape both come through here.
    fn close_queue_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.queue_modal = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Keep the window title on the playing track: "artist - title - rox"
    /// while something plays, the plain app name otherwise. Untagged files
    /// fall back to their file name, same as the track info readout.
    fn refresh_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.state.player.read(cx).now_playing().map(|now| now.key);
        if key == self.titled_track {
            return;
        }
        let title = match &key {
            Some(key) => {
                let meta = self.state.library.read(cx).meta_for_key(key);
                let track = meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| {
                    key.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| key.path.display().to_string())
                });
                match meta.map(|m| m.artist).filter(|a| !a.is_empty()) {
                    Some(artist) => format!("{artist} - {track} - rox"),
                    None => format!("{track} - rox"),
                }
            }
            None => "rox".into(),
        };
        rox_panel_api::windows::set_window_title(window, &title);
        self.titled_track = key;
    }

    /// Route OS-handed files into the shared player. The launch path
    /// (`rox song.flac` and the .desktop actions) comes through here after
    /// the window's player exists; paths are already filtered to decodable
    /// audio.
    /// Play replaces the restored session so double-clicking a file starts it;
    /// enqueue appends. The player is path-based, so files outside the library
    /// play fine.
    pub fn open_paths(
        &mut self,
        mode: rox_library::open_files::LaunchMode,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        play_launch_paths(&self.state, mode, paths, cx);
    }

    /// Play files or tracks dropped onto the window body now, filtered to
    /// decodable audio. A drop onto the window reads as "play this", so it
    /// splices in right after the current track and jumps to it, keeping the
    /// rest of the queue behind it. Dropping onto the queue panel adds to the
    /// queue instead; that panel's own handler catches the drop first. An OS
    /// file open (the .desktop default) still replaces the session, that path
    /// runs through open_paths, not here.
    fn play_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.play_keys(loose_keys(paths), cx);
    }

    /// The same for a drag out of the library, which already has keys and
    /// so never loses a cue track's number on the way over.
    fn play_keys(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_now(keys, cx));
    }

    /// Add dropped files or tracks to the up-next queue, filtered to decodable
    /// audio. The Add to queue drop zone routes here.
    fn queue_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.queue_keys(loose_keys(paths), cx);
    }

    /// The same for a library drag, keys and all.
    fn queue_keys(&mut self, keys: Vec<TrackKey>, cx: &mut Context<Self>) {
        if keys.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(keys, cx));
    }

    /// The Play now / Add to queue drop zones, shown only while an audio
    /// payload is dragged: a file from the OS (ExternalPaths) or a track from
    /// the library (PlayDrag). Other drags (panel docking, queue reorder)
    /// leave them hidden. Rendered as the top layer so the drop always lands
    /// here: an occluded window-root target misses it because the panels
    /// block the hit test.
    fn drop_zones_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !cx.active_drag_is::<ExternalPaths>() && !cx.active_drag_is::<PlayDrag>() {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .p(tokens::SPACE_MD)
                .bg(rgba(0x00000055))
                .child(self.drop_zone(
                    rox_i18n::t!("workspace-drop-play-now"),
                    icons::PLAY,
                    true,
                    cx,
                ))
                .child(self.drop_zone(
                    rox_i18n::t!("workspace-drop-add-queue"),
                    icons::LIST_MUSIC,
                    false,
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// One drop zone card. `play_now` true plays the drop after the current
    /// track and jumps to it; false appends it to the queue. Both accept a
    /// file from the OS and a track dragged from the library.
    fn drop_zone(
        &self,
        label: impl Into<SharedString>,
        icon: &'static str,
        play_now: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let card = div()
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .rounded(tokens::RADIUS)
            .border_2()
            .border_color(palette::border_light())
            .bg(palette::bg_menu_opaque())
            .text_color(palette::text_muted())
            .child(Icon::default().path(icon))
            .child(div().text_lg().child(label.into()))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .border_color(palette::accent())
                    .bg(palette::bg_control_hover_opaque())
            })
            .drag_over::<PlayDrag>(|style, _, _, _| {
                style
                    .border_color(palette::accent())
                    .bg(palette::bg_control_hover_opaque())
            });
        if play_now {
            card.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.play_dropped(paths.paths().to_vec(), cx);
            }))
            .on_drop(cx.listener(|this, drag: &PlayDrag, _, cx| {
                this.play_keys(drag.keys.to_vec(), cx);
            }))
        } else {
            card.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.queue_dropped(paths.paths().to_vec(), cx);
            }))
            .on_drop(cx.listener(|this, drag: &PlayDrag, _, cx| {
                this.queue_keys(drag.keys.to_vec(), cx);
            }))
        }
    }

    /// Register the OS media service over this window's state. The hand-off
    /// target when the window that owned the service closes with this one
    /// still open. The D-Bus name is per-process, so the old owner has to
    /// release it first.
    fn install_media(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.media = MediaSession::new(self.state.clone(), window, cx);
    }

    /// Hand the OS media service off this window, freeing the per-process
    /// D-Bus name. The service itself only ends when whoever takes it here
    /// drops it.
    fn take_media(&mut self) -> Option<Entity<MediaSession>> {
        self.media.take()
    }

    /// Push the play state to the tray's Play/Pause label, from the window
    /// that owns the media service so several windows don't fight over it.
    /// Gated in the tray, so player notifies don't turn into D-Bus writes.
    fn publish_tray(&mut self, cx: &mut Context<Self>) {
        if self.media.is_none() {
            return;
        }
        let (has_track, playing) = {
            let player = self.state.player.read(cx);
            (player.now_playing().is_some(), player.is_playing())
        };
        tray::set_playing(has_track, playing, cx);
    }

    /// Debounced persist: wait out [`SAVE_DEBOUNCE`] of quiet, then dump.
    /// Replacing the task cancels the previous timer.
    fn save_layout_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| this.persist(window, cx))
                .ok();
        }));
    }

    /// Dump the dock layout and the window frame into the settings file.
    /// With several windows open the last writer wins; the file records the
    /// layout most recently touched.
    pub(crate) fn persist(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.save_task = None;
        let layout = self.dock_dump(cx).ok();
        let bounds = window.window_bounds();
        let frame = bounds.get_bounds();
        let window_state = WindowState {
            x: frame.origin.x.into(),
            y: frame.origin.y.into(),
            width: frame.size.width.into(),
            height: frame.size.height.into(),
            maximized: matches!(bounds, WindowBounds::Maximized(_)),
        };
        // The playing track is stored as its library id, for the launch
        // restore. Nothing playing, or a file outside the library, clears
        // it: the next launch starts cold.
        let library = self.state.library.read(cx);
        let last_track = self.state.player.read(cx).now_playing().and_then(|now| {
            let id = library.id_for_key(&now.key)?;
            Some(LastTrack {
                id,
                sub: now.key.sub,
                position_secs: now.position_secs,
            })
        });
        // The whole queue goes in too, as library ids so a path change
        // doesn't break it, keeping each entry's explicit flag and the
        // audible cursor. A file outside the library drops from the order;
        // the cursor tracks the last kept entry at or before it so it stays
        // on the playing track. Everything gone (or nothing playing) clears
        // it and the single-track fallback above does the restore.
        let last_queue = self.state.player.read(cx).queue_state().and_then(
            |(entries, cursor, position_secs)| {
                let mut tracks = Vec::with_capacity(entries.len());
                let mut new_cursor = 0;
                for (i, (key, explicit)) in entries.iter().enumerate() {
                    if let Some(id) = library.id_for_key(key) {
                        if i <= cursor {
                            new_cursor = tracks.len();
                        }
                        tracks.push(QueuedTrack {
                            id,
                            sub: key.sub,
                            explicit: *explicit,
                        });
                    }
                }
                (!tracks.is_empty()).then_some(QueueState {
                    entries: tracks,
                    cursor: new_cursor,
                    position_secs,
                })
            },
        );
        Settings::update(move |s| {
            s.look.layout = layout;
            s.windows.main = Some(window_state);
            s.session.last_track = last_track;
            s.session.last_queue = last_queue;
        });
    }

    fn add_center(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The dock's own add-to-center always targets the initial tabs item,
        // but drags and closes can empty that tab panel out of the tree.
        // Add to it while it still shows, otherwise to the newest live tab
        // panel, and failing both put the original one back at the top of
        // the root stack, above the transport row.
        let tabs = if self.center_tabs.read(cx).visible(cx) {
            self.center_tabs.clone()
        } else if let Some(tabs) = self.state.tab_hosts.read(cx).last_live(cx) {
            tabs
        } else {
            let tabs_view: Arc<dyn PanelView> = Arc::new(self.center_tabs.clone());
            let weak_dock = self.dock.downgrade();
            self.stack.update(cx, |stack, cx| {
                stack.insert_panel_before(tabs_view, 0, None, weak_dock, window, cx);
            });
            self.center_tabs.clone()
        };
        tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
    }

    /// New audio and transport panels join the transport row as their own
    /// tab group at the end. A new group rather than a new tab, so they end
    /// up next to the transport pieces instead of hiding one. The library
    /// stays a center panel: it needs the tall area, and keeping additions
    /// on the center path preserves the recovery route when every center
    /// panel has been closed or popped out.
    fn add_bottom(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak_dock = self.dock.downgrade();
        // The row removes itself from the tree when its last group closes,
        // so put it back at the bottom of the root stack first. A no-op
        // while it's still attached: stacks skip panels they already hold.
        let row: Arc<dyn PanelView> = Arc::new(self.bottom_stack.clone());
        self.stack.update(cx, |stack, cx| {
            stack.add_panel(
                row,
                Some(px(TRANSPORT_ROW_H)),
                weak_dock.clone(),
                window,
                cx,
            );
        });
        let (_, tabs) = tabs_item(vec![panel], &weak_dock, window, cx);
        self.bottom_stack.update(cx, |stack, cx| {
            stack.add_panel(Arc::new(tabs), None, weak_dock, window, cx);
        });
    }

    /// The search bar joins the top of the root stack as its own thin tab
    /// group across the whole window, above the center panels. A search bar
    /// reads as a strip rather than a tall tile, so it goes in sized to just
    /// its input row rather than splitting a center panel's height.
    fn add_top(&mut self, panel: Arc<dyn PanelView>, window: &mut Window, cx: &mut Context<Self>) {
        let weak_dock = self.dock.downgrade();
        let (_, tabs) = tabs_item(vec![panel], &weak_dock, window, cx);
        self.stack.update(cx, |stack, cx| {
            stack.insert_panel_before(
                Arc::new(tabs),
                0,
                Some(px(SEARCH_BAR_H)),
                weak_dock,
                window,
                cx,
            );
        });
    }

    /// Whether the dock shows no panels at all: every stack walked down
    /// to tab groups and all of them empty. The face an Empty Window
    /// opens with, or closing the last panel leaves behind.
    fn dock_is_empty(&self, cx: &App) -> bool {
        fn node_empty(node: &Arc<dyn PanelView>, cx: &App) -> bool {
            let view = node.view();
            if let Ok(stack) = view.clone().downcast::<StackPanel>() {
                let children = stack.read(cx).panels().to_vec();
                return children.iter().all(|child| node_empty(child, cx));
            }
            if let Ok(tabs) = view.downcast::<TabPanel>() {
                return tabs.read(cx).panels().is_empty();
            }
            false
        }
        let root = self.dock.read(cx).items().view();
        node_empty(&root, cx)
    }

    /// The empty dock's launcher, floated mid-window: an empty tab group
    /// renders to nothing, so without this a blank window gives no way in.
    /// The rox mark heads it, then the panel catalog, one titled section
    /// per group. The whole-look pickers are in the welcome window's
    /// quick-start tiles now; this stays the piece-by-piece way in.
    fn empty_hint(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            // An empty dock paints no surface of its own, so without this
            // the launcher's copy would be drawn straight onto the backdrop
            // art. The same page surface the settings pages use: opaque at
            // full surface opacity, thinning with the rest.
            .bg(palette::bg_elevated())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            // A way back to the welcome window's quick-start tiles, since
            // this launcher replaced it as the empty window's face.
            .child(
                div()
                    .absolute()
                    .top(tokens::SPACE_SM)
                    .right(tokens::SPACE_SM)
                    .child(panel::icon_control(
                        icons::INFO,
                        palette::text_muted(),
                        panel::Tip::keyed(
                            "open-welcome-hint",
                            rox_i18n::t!("workspace-launcher-open-welcome"),
                        ),
                        |this: &mut Self, cx| {
                            crate::startup::welcome_window::open(this.state.clone(), cx);
                        },
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .max_w(px(600.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .pb(tokens::SPACE_XS)
                            .child(
                                svg()
                                    .path(icons::LOGO)
                                    .size(px(40.))
                                    .text_color(palette::text()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_color(palette::text())
                                            .child(rox_i18n::t!("workspace-launcher-title")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(palette::text_muted())
                                            .child(rox_i18n::t!("workspace-launcher-hint")),
                                    )
                                    // The fallback notice: this window is
                                    // empty because a layout wouldn't
                                    // restore, not because the user asked.
                                    // Accent rather than a status red: the
                                    // app recovered, this is emphasis.
                                    .when_some(self.layout_error.clone(), |d, message| {
                                        d.child(
                                            div()
                                                .pt(tokens::SPACE_XS)
                                                .text_xs()
                                                .text_color(palette::accent())
                                                .child(message),
                                        )
                                    }),
                            ),
                    )
                    // The panel catalog: one titled section per group; the
                    // bare center run reads under a plain "Panels".
                    .children(catalog::sections().map(|section| {
                        let tiles = section.panels.iter().map(|def| {
                            launcher_tile(
                                rox_i18n::t!(def.label),
                                def.icon,
                                catalog::supports_signals(def),
                                cx.listener(move |this, _, window, cx| {
                                    this.run(MenuAction::OpenPanel(def), window, cx);
                                }),
                            )
                        });
                        let group_label = section
                            .group
                            .map(|(label, _)| rox_i18n::t!(label))
                            .unwrap_or_else(|| rox_i18n::t!("menu-panels"));
                        launcher_section(group_label, tiles)
                    }))
                    // Under the whole catalog: the other way back to the
                    // welcome window's quick-start, spelled out for anyone who
                    // read past the panels without spotting the corner info.
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_XS)
                            .pt(tokens::SPACE_XS)
                            .text_xs()
                            .text_color(palette::text_muted())
                            .cursor_pointer()
                            .hover(|d| d.text_color(palette::text()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    crate::startup::welcome_window::open(this.state.clone(), cx);
                                }),
                            )
                            .child(rox_i18n::t!("workspace-launcher-need-help")),
                    ),
            )
    }

    /// The mini-player toggle, at the menubar's left edge before the menus.
    /// Shows whenever a mini layout is assigned; the glyph flips to say
    /// which way the next click goes. Built inline rather than through
    /// [`panel::icon_control`] because the swap needs the window the icon
    /// helper doesn't pass, and it reads like a menu button beside them.
    fn mini_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.mini_layout.as_ref()?;
        // The glyph says which way the click goes and the tip says it in
        // words, since two arrows pointing in and two pointing out are a
        // coin flip to anyone who hasn't clicked it before.
        let (icon, tip) = if self.on_mini() {
            (icons::MAXIMIZE, rox_i18n::t!("workspace-mini-tip-back"))
        } else {
            (icons::MINIMIZE, rox_i18n::t!("workspace-mini-tip-shrink"))
        };
        Some(
            panel::Tip::keyed("mini-toggle", tip).apply(
                div()
                    .h_full()
                    .px(tokens::SPACE_MD)
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_menu_hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.toggle_mini(window, cx)),
                    )
                    .child(
                        svg()
                            .path(icon)
                            .size(px(14.))
                            .text_color(palette::text_muted()),
                    ),
            ),
        )
    }

    /// The layout save/apply dialog, floated over the window on its own
    /// occluding layer. The save card sets the `SearchInput` key context
    /// so space and arrows type into the name field instead of driving
    /// playback, the search boxes' trick.
    fn layout_dialog_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.layout_dialog.as_ref()?;
        let card = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .w(px(320.))
            .p(tokens::SPACE_MD)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude();
        let card = match dialog {
            LayoutDialog::Save(input) => card
                .key_context("SearchInput")
                .child(div().child(rox_i18n::t!("workspace-dialog-save-layout-title")))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-save"),
                            true,
                            cx.listener(|this, _, window, cx| this.commit_save(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmOverwrite(name) => card
                .child(div().child(rox_i18n::t!(
                    "workspace-dialog-overwrite-title",
                    name = name.as_str()
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("workspace-layout-overwrite-body")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-overwrite"),
                            true,
                            cx.listener(|this, _, window, cx| this.overwrite_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmApply(name) => card
                .child(div().child(rox_i18n::t!(
                    "workspace-dialog-apply-title",
                    name = name.as_str()
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("workspace-layout-apply-body")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-apply"),
                            true,
                            cx.listener(|this, _, window, cx| this.apply_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::SaveWorkspace(input) => card
                .key_context("SearchInput")
                .child(div().child(rox_i18n::t!("workspace-dialog-save-workspace-title")))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-save"),
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.commit_save_workspace(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmOverwriteWorkspace(name) => card
                .child(div().child(rox_i18n::t!(
                    "workspace-dialog-overwrite-title",
                    name = name.as_str()
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("workspace-overwrite-body")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-overwrite"),
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.overwrite_workspace_confirmed(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmApplyWorkspace {
                card: bundle_card,
                imported,
            } => {
                let shaders = bundle_card.shader_line();
                let screen = bundle_card.screen_shader.clone();
                // Whether the yes splits in two. Code nobody has agreed to
                // splits it, and so does a look that just runs shaders,
                // however many times it's been applied before.
                let split = bundle_card.splits_apply();
                let line = |text: SharedString| {
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(text)
                };
                card
                    // The shader list and the screen shader's hotkey line both
                    // need the room; a plain apply keeps the dialogs' shared
                    // width.
                    .when(split || screen.is_some(), |d| d.w(px(380.)))
                    .child(div().child(if *imported {
                        rox_i18n::t!(
                            "workspace-apply-imported-title",
                            name = bundle_card.name.as_str()
                        )
                    } else {
                        rox_i18n::t!(
                            "workspace-dialog-apply-title",
                            name = bundle_card.name.as_str()
                        )
                    }))
                    .children(bundle_card.byline.clone().map(line))
                    .children(bundle_card.description.clone().map(line))
                    .child(line(if *imported {
                        rox_i18n::t!("workspace-apply-imported-body")
                    } else {
                        rox_i18n::t!("workspace-apply-body")
                    }))
                    // A screen shader covers the whole window, so it gets said
                    // before the apply rather than asked about after, and the
                    // way back off comes with it.
                    .children(screen.clone().map(line))
                    .children(screen.map(|_| {
                        kbd_line([
                            Seg::Text(rox_i18n::t!("workspace-screen-shader-hint-before")),
                            Seg::Key(chord("Shift+X")),
                            Seg::Text(rox_i18n::t!("workspace-hint-or")),
                            Seg::Key(rox_i18n::t!("menu-window")),
                            Seg::Text(rox_i18n::t!("workspace-hint-then")),
                            Seg::Key(rox_i18n::t!("menu-overlay-shader")),
                        ])
                        .text_xs()
                    }))
                    .children(shaders.clone().map(line))
                    // Shaders that came with a look are somebody else's code,
                    // so the yes that runs them says so, and the yes that
                    // doesn't is right beside it. Once they're agreed to the
                    // question is only about the look, and the line says that
                    // instead.
                    .children(split.then(|| {
                        line(if shaders.is_some() {
                            rox_i18n::t!("workspace-apply-shaders-approve-body")
                        } else {
                            rox_i18n::t!("workspace-apply-shaders-plain-body")
                        })
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(tokens::SPACE_SM)
                            .child(dialog_button(
                                if *imported {
                                    rox_i18n::t!("workspace-dialog-not-now")
                                } else {
                                    rox_i18n::t!("workspace-dialog-cancel")
                                },
                                false,
                                cx.listener(|this, _, window, cx| {
                                    this.close_layout_dialog(window, cx)
                                }),
                            ))
                            .child(dialog_button(
                                if split {
                                    rox_i18n::t!("workspace-dialog-without-shaders")
                                } else {
                                    rox_i18n::t!("workspace-dialog-apply")
                                },
                                !split,
                                cx.listener(|this, _, window, cx| {
                                    this.apply_workspace_confirmed(ApplyShaders::Skip, window, cx)
                                }),
                            ))
                            .children(split.then(|| {
                                dialog_button(
                                    if shaders.is_some() {
                                        rox_i18n::t!("workspace-dialog-approve-apply")
                                    } else {
                                        rox_i18n::t!("workspace-dialog-with-shaders")
                                    },
                                    true,
                                    cx.listener(|this, _, window, cx| {
                                        this.apply_workspace_confirmed(
                                            ApplyShaders::Wear,
                                            window,
                                            cx,
                                        )
                                    }),
                                )
                            })),
                    )
            }
            LayoutDialog::ConfirmCloseLocked { name, .. } => card
                .child(div().child(rox_i18n::t!(
                    "workspace-dialog-close-title",
                    name = name.as_ref()
                )))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("workspace-panel-locked-close-body")),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-cancel"),
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            rox_i18n::t!("workspace-dialog-close"),
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.close_locked_confirmed(window, cx)
                            }),
                        )),
                ),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000066))
                .child(card)
                .into_any_element(),
        )
    }

    /// The queue modal: the queue panel floated over the workspace on a
    /// dimming scrim. The card occludes, so a click on it stays on the queue;
    /// a click on the scrim outside it closes, as does Escape, which bubbles
    /// up from the focused queue panel (its own key handler leaves Escape
    /// alone). Sized fixed so the queue keeps a definite height off the dock.
    fn queue_modal_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let queue = self.queue_modal.clone()?;
        let card = div()
            .w(px(640.))
            .h(px(520.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_queue_modal(window, cx);
                }
            }))
            .child(queue);
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000066))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_queue_modal(window, cx)),
                )
                .child(card)
                .into_any_element(),
        )
    }
}

/// Snap every float in a serialized dock dump to the shortest decimal that
/// round-trips through f32. The dump's sizes and panel configs are all f32,
/// but [`serde_json::to_value`] widens them to f64 and bakes in the expansion
/// noise: 0.05 comes back as 0.05000000074505806, a splitter at 584.31px as
/// 584.3106079101562. Walking the value once strips that back to clean numbers
/// without touching the actual value, since f32's Display is already the
/// shortest round-tripping decimal. Integers serialize as i64/u64 and are
/// exact, so they're left alone.
pub(crate) fn denoise_f32(value: &mut serde_json::Value) {
    use serde_json::Value;
    match value {
        // Integers deserialize as i64/u64 and are exact; only widened f64s need
        // snapping, so the guard skips the integer numbers.
        Value::Number(n) if n.as_i64().is_none() && n.as_u64().is_none() => {
            if let Some(clean) = n
                .as_f64()
                .and_then(|f| (f as f32).to_string().parse::<f64>().ok())
                .and_then(serde_json::Number::from_f64)
            {
                *n = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(denoise_f32),
        Value::Object(map) => map.values_mut().for_each(denoise_f32),
        _ => {}
    }
}

/// Resize the window to a preset's stored size, floored at the window
/// minimum. A bad or zero size in a preset would otherwise collapse the
/// window to nothing on a layout swap or mini toggle.
fn resize_clamped(window: &mut Window, size: LayoutSize) {
    window.resize(gpui::size(
        px(size.width).max(rox_core::settings::MIN_WINDOW_SIZE.width),
        px(size.height).max(rox_core::settings::MIN_WINDOW_SIZE.height),
    ));
}

/// The window's content size in logical pixels, for storing with a layout
/// preset. A maximized window reports its restore size, the size the preset
/// makes sense to reopen at.
fn window_size(window: &Window) -> LayoutSize {
    let size = window.window_bounds().get_bounds().size;
    LayoutSize {
        width: size.width.into(),
        height: size.height.into(),
    }
}

/// Which side the flyout hanging off the surface at `level` should open:
/// right of its parent by default, left when the window is out of room on
/// the right, and still left while a flipped chain has room to keep going,
/// so a deep flyout never doubles back over its grandparent. `surfaces` are
/// the menu surfaces' painted bounds from the last frame, root first, one
/// per open level; the estimate stands in for the flyout's own width, which
/// is unknown until it paints. Shared by the menubar and the menu panel.
pub(crate) fn flyout_leftward(
    surfaces: &[Option<Bounds<Pixels>>],
    level: usize,
    viewport_w: Pixels,
) -> bool {
    let Some(&Some(bounds)) = surfaces.get(level) else {
        return false;
    };
    let est = px(240.);
    if bounds.origin.x < est {
        return false;
    }
    let parent_flipped = level
        .checked_sub(1)
        .and_then(|parent| surfaces.get(parent))
        .and_then(|parent| parent.as_ref())
        .is_some_and(|parent| bounds.origin.x < parent.origin.x);
    parent_flipped || bounds.right() + est > viewport_w
}

/// A flyout's horizontal anchor on its row, the side
/// [`flyout_leftward`] picked.
pub(crate) fn flyout_side(d: Div, leftward: bool) -> Div {
    if leftward {
        d.right_full()
    } else {
        d.left_full()
    }
}

/// A muted heading over a divider, grouping the rows below it in a dropdown.
/// `label` is an i18n key, not display text. The table that builds these
/// rows is `const` and can't call a translator, so the key travels
/// unresolved until it gets here.
pub(crate) fn menu_section(label: &'static str) -> Div {
    div()
        .mt(tokens::SPACE_XS)
        .pt(tokens::SPACE_XS)
        .px(tokens::SPACE_MD)
        .border_t_1()
        .border_color(palette::border())
        .text_xs()
        .text_color(palette::text_muted())
        .child(rox_i18n::t!(label))
}

/// A launcher row: a centered wrap of tiles.
fn tile_row() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(tokens::SPACE_SM)
}

/// A titled launcher block: a centered header over its wrap of tiles. The
/// headers stay muted and small so the tiles carry the weight.
fn launcher_section(header: impl Into<SharedString>, tiles: impl IntoIterator<Item = Div>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(header.into()),
        )
        .child(tile_row().children(tiles))
}

/// A launcher tile: an icon-and-label chip that opens a panel or applies
/// a layout with one click. `signals` trails the pool's glyph, the mark
/// the menus put on the same panels.
fn launcher_tile(
    label: impl Into<SharedString>,
    icon: &'static str,
    signals: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .bg(palette::bg_control())
        .hover(|d| d.bg(palette::bg_control_hover()))
        .on_mouse_down(MouseButton::Left, on_click)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(
            svg()
                .path(icon)
                .size_3p5()
                .text_color(palette::text_muted()),
        )
        .child(label.into())
        .when(signals, |d| {
            d.child(
                svg()
                    .path(icons::AUDIO_WAVEFORM)
                    .size_3()
                    .text_color(palette::text_faint()),
            )
        })
}

/// A dialog button: the primary one reads as a filled accent control, the
/// rest as plain controls.
fn dialog_button(
    label: impl Into<SharedString>,
    primary: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
                    .hover(|d| d.opacity(0.9))
            } else {
                d.bg(palette::bg_control())
                    .hover(|d| d.bg(palette::bg_control_hover()))
            }
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.into())
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The screen shader's hot reload and signal feed run off the render
        // loop; it keeps its own frame requests going while audio moves.
        self.drive_post_shader(window, cx);
        // A hidden menubar comes back while alt is held, and stays while a
        // dropdown is open so releasing alt can't strand one barless.
        let menubar_hidden = settings::hide_menubar();
        // A pin from the last time the bar was hidden has nothing to hold up
        // once the bar is docked again.
        if !menubar_hidden {
            self.menubar_pinned = false;
            self.menubar_touched = false;
        }
        // Alt reveals the hidden bar, but Alt+drag is the compositor's
        // window move/resize; suppress the reveal while a button is down so
        // the overlay is never in front of the drag. An open menu keeps
        // it up regardless (that press landed on a menu, not a drag), and so
        // does a double-tap pin, which is the way to click the bar without a
        // modifier changing what the click means.
        let menubar_revealed = self.menubar_pinned
            || self.open_menu.is_some()
            || (window.modifiers().alt && !self.pointer_down);
        // Every panel in this window renders under its player's art tint,
        // and the window claims the one widget theme while it holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        let dock_empty = self.dock_is_empty(cx);
        panel::workspace_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .track_focus(&self.focus)
                .key_context("Workspace")
                .on_action(cx.listener(|this, _: &TogglePlayback, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.toggle_pause());
                }))
                .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.seek_by(-5.0));
                }))
                .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, _| player.seek_by(5.0));
                }))
                .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                    crate::settings::window::open(
                        this.state.clone(),
                        cx.entity().downgrade(),
                        window.window_handle(),
                        this.dock.clone(),
                        cx,
                    );
                }))
                .on_action(cx.listener(|this, _: &OpenStats, _, cx| {
                    crate::stats_window::open(this.state.clone(), cx);
                }))
                .on_action(cx.listener(|this, _: &OpenQuickPlay, window, cx| {
                    this.toggle_quick_play(window, cx);
                }))
                .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                    this.dock.update(cx, |dock, cx| {
                        dock.focus_panel_named("search", window, cx);
                    });
                }))
                .on_action(cx.listener(|this, _: &ToggleZoom, window, cx| {
                    this.dock
                        .update(cx, |dock, cx| dock.toggle_zoom_active(window, cx));
                }))
                // Escape backs out of a zoomed panel. A raw listener, not a
                // binding: bindings win over key listeners, and the escape
                // ladders (search boxes, quick-play) are listeners that stop
                // propagation, so a binding here would steal their escape.
                // This runs last in the bubble, so it only sees what they let
                // through.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key != "escape" || event.keystroke.modifiers.modified() {
                        return;
                    }
                    if this.layout_dialog.is_some() {
                        this.close_layout_dialog(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    if this.unpin_menubar(cx) {
                        cx.stop_propagation();
                        return;
                    }
                    if this.dock.update(cx, |dock, cx| dock.zoom_out(window, cx)) {
                        cx.stop_propagation();
                    }
                }))
                // Any keystroke under a held Alt makes it a chord rather than
                // the tap that pins the bar. Captured so a panel eating the key
                // can't hide it; bare modifiers arrive as a modifiers change,
                // not a keystroke, so Alt itself never gets here.
                .capture_key_down(cx.listener(|this, _: &KeyDownEvent, _, _| {
                    this.cancel_alt_tap();
                }))
                // Quit bypasses the window close hook, so dump the layout and
                // frame here or a pending debounce and any window move since
                // the last save are lost.
                .on_action(cx.listener(|this, _: &Quit, window, cx| {
                    this.persist(window, cx);
                    cx.quit();
                }))
                // Alt reveals a hidden menubar, so modifier flips repaint. Gated
                // on the setting so the common case stays free of repaints. The
                // tap tracking runs either way, since the bar can be hidden
                // between the two taps.
                .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                    this.note_modifiers(event.modifiers, cx);
                    if settings::hide_menubar() {
                        cx.notify();
                    }
                }))
                // Track the mouse button in the capture phase so the
                // alt-revealed bar can duck a window move/resize drag. Capture
                // beats the occluding overlay and any panel that eats the
                // press; only the alt-reveal path cares, so repaint just there.
                .capture_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    // A press under a held Alt is a drag, not a tap.
                    this.cancel_alt_tap();
                    // A press anywhere but the bar itself is the user going
                    // back to the app, so the pin has done its job. The bar
                    // is full width at the top, so the height is the whole
                    // test; a dropdown hangs below it and counts as outside,
                    // which is what closing the menu should do anyway.
                    if event.position.y > px(MENU_BAR_H) {
                        this.unpin_menubar(cx);
                    }
                    if !this.pointer_down {
                        this.pointer_down = true;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                    if this.pointer_down {
                        this.pointer_down = false;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                // A compositor-driven Alt+drag can swallow the release, leaving
                // the flag stuck down. Any later move with no button held
                // reconciles it, so the next Alt press reveals the bar again.
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if this.pointer_down && event.pressed_button.is_none() {
                        this.pointer_down = false;
                        if settings::hide_menubar() && this.open_menu.is_none() {
                            cx.notify();
                        }
                    }
                }))
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // The app font cascades from here into the menubar, dock, and
                // every panel; a panel's own font override layers over it in
                // the themed wrapper. None follows the platform default.
                .when_some(settings::app_font(), |d, font| d.font_family(font))
                // The backdrop paints first, under the menubar and dock; how
                // much shows through is the surfaces' call (ADR 10's strength
                // scalar).
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                // The screen shader's cursor watch. It has to be registered
                // from a paint, and the driver above runs in render, so a
                // probe that draws nothing does it. Without it a pass
                // that faded its cursor effect out and parked its frames
                // waits for something else to dirty the window before it
                // notices the hand is back on the mouse.
                .when(
                    self.post_shader
                        .as_ref()
                        .is_some_and(|driver| driver.reads_cursor),
                    |d| {
                        d.child(
                            canvas(
                                |_, _, _| {},
                                |_, _, window: &mut Window, _| panel::shader::watch_cursor(window),
                            )
                            .absolute()
                            .size_0(),
                        )
                    },
                )
                .when(!menubar_hidden, |d| d.child(self.menubar(window, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(self.dock.clone())
                        // The hint floats over the dock area only, so it
                        // never covers the menubar or the overlays below.
                        .when(dock_empty, |d| d.child(self.empty_hint(cx))),
                )
                // A hidden bar floats over the dock while revealed, so the
                // layout never shifts under it. After the dock child so it
                // paints on top; occlude keeps its clicks off what it covers.
                .when(menubar_hidden && menubar_revealed, |d| {
                    d.child(
                        div()
                            // Stateful for the hover below, which clears a
                            // pinned bar.
                            .id("menubar-reveal")
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .occlude()
                            // A pinned bar clears itself once the pointer has
                            // been over it and left again.
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                this.note_menubar_hover(*hovered, cx);
                            }))
                            .child(self.menubar(window, cx)),
                    )
                })
                // The quick-play modal floats over everything on an occluding
                // layer, so a click outside it dismisses without also landing
                // on whatever is underneath. Not deferred: it's the last
                // child so it already paints on top, and the search box's
                // suggestion popover defers itself; gpui panics on a
                // defer_draw inside a deferred draw. `overlay_phase` buys
                // the rest of that deal: painting last puts it over the
                // dock's primitives, but a panel's region shader pass runs at
                // the end of the frame and would swallow it, so it has to
                // record in the same draw-order range menus and tooltips do.
                .when_some(self.quick_play.clone(), |d, modal| {
                    d.child(overlay_phase(
                        div()
                            .absolute()
                            .inset_0()
                            .occlude()
                            .flex()
                            .flex_col()
                            .items_center()
                            .pt(px(96.))
                            .child(modal),
                    ))
                })
                // The layout save/apply dialog floats over everything, same as
                // quick-play and for the same reasons: last child, not deferred.
                .children(self.layout_dialog_overlay(cx).map(overlay_phase))
                // The queue modal floats the same way, last so it paints over
                // the dock.
                .children(self.queue_modal_overlay(cx).map(overlay_phase))
                // The Play now / Add to queue drop zones. Last child so they
                // go on top of every panel, which also makes them the topmost
                // hitbox: an occluded workspace-root drop target would miss the
                // drop entirely (panels block the hit test).
                .children(self.drop_zones_overlay(cx).map(overlay_phase))
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::denoise_f32;
    use serde_json::json;

    #[test]
    fn denoise_strips_widened_f32_tails() {
        // The exact f64 expansions serde_json::to_value produces from f32
        // sizes and configs, the way they landed in saved layouts.
        let mut v = json!({
            "sizes": [584.3106079101562f64, 429.8237609863281f64],
            "config": {
                "cap_gravity": 0.05000000074505806f64,
                "line_spacing": 1.899999976158142f64,
            },
        });
        denoise_f32(&mut v);
        // Re-serialized, the numbers read as their clean f32 form.
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("584.3106"), "{s}");
        assert!(s.contains("429.82376"), "{s}");
        assert!(s.contains("0.05"), "{s}");
        assert!(s.contains("1.9"), "{s}");
        assert!(!s.contains("0.05000000"), "{s}");
        assert!(!s.contains("584.3106079"), "{s}");
    }

    #[test]
    fn denoise_leaves_integers_and_strings_alone() {
        let mut v = json!({
            "active_index": 2,
            "axis": 1,
            "panel_name": "album grid",
            "count": 50000,
        });
        let before = v.clone();
        denoise_f32(&mut v);
        // Integers deserialize as i64/u64, so nothing is touched.
        assert_eq!(v, before);
    }

    #[test]
    fn denoise_is_idempotent() {
        let mut v = json!({ "w": 0.05000000074505806f64, "sizes": [713.17626953125f64] });
        denoise_f32(&mut v);
        let once = v.clone();
        denoise_f32(&mut v);
        assert_eq!(v, once);
    }
}

#[cfg(test)]
mod shader_feed_tests {
    use super::*;
    use rox_viz::signal::Source;
    use rox_viz::AudioFeed;

    /// A hub with one band signal, ticked once so the engine has a slot
    /// to read. Silent: what's being checked here is which path fills the
    /// slots, and a route reads its Quiet end at silence, which makes the
    /// two paths tell themselves apart with no audio at all.
    fn silent_hub() -> (SignalHub, u64) {
        let hub = SignalHub::new(Vec::new());
        let (id, _) = hub.add(
            Source::Band {
                lo: 30.0,
                hi: 120.0,
            },
            0.0,
        );
        hub.tick(&AudioFeed::new(), None);
        (hub, id)
    }

    /// Both paths in one test: they share a process-wide static, and two
    /// tests setting it would race each other in the same binary.
    #[test]
    fn routes_take_over_the_feed_and_nothing_routed_keeps_pool_order() {
        let (hub, id) = silent_hub();
        set_post_shader_routes(Vec::new());
        assert_eq!(post_shader_signals(&hub), [0.0; panel::shader::SLOTS]);

        set_post_shader_routes(vec![Route {
            enabled: true,
            signal: id,
            target: panel::shader::slot_target(3),
            from: 0.5,
            to: 1.0,
        }]);
        let signals = post_shader_signals(&hub);
        // The route's Quiet end, which the pool-order feed has no way to
        // produce: it can only ever hand a slot the signal's own value.
        assert!(
            (signals[3] - 0.5).abs() < 1e-4,
            "slot 3 should read the route's quiet end, got {}",
            signals[3]
        );
        // And slot 0 no longer takes pool signal 0 just for being first.
        assert_eq!(signals[0], 0.0);

        // A route pointing at a signal the pool never held leaves its
        // slot alone rather than falling back to the pool order.
        set_post_shader_routes(vec![Route {
            enabled: true,
            signal: id + 99,
            target: panel::shader::slot_target(1),
            from: 0.5,
            to: 1.0,
        }]);
        assert_eq!(post_shader_signals(&hub), [0.0; panel::shader::SLOTS]);

        // Hand-set values, the panel rule on the app-wide list: they stand
        // where nothing else fills the slot, and a route wins while it's there.
        set_post_shader_manual(vec![(5, 0.75), (3, 0.2)]);
        set_post_shader_routes(vec![Route {
            enabled: true,
            signal: id,
            target: panel::shader::slot_target(3),
            from: 0.5,
            to: 1.0,
        }]);
        let signals = post_shader_signals(&hub);
        assert!(
            (signals[5] - 0.75).abs() < 1e-4,
            "an unrouted hand-set slot reads its value, got {}",
            signals[5]
        );
        assert!(
            (signals[3] - 0.5).abs() < 1e-4,
            "the route wins over the hand-set value, got {}",
            signals[3]
        );

        // And the legacy pool-order feed steps around a hand-set slot:
        // pool signal 0 reads silent zero, which would overwrite the knob
        // without the skip.
        set_post_shader_routes(Vec::new());
        set_post_shader_manual(vec![(0, 0.9)]);
        let signals = post_shader_signals(&hub);
        assert!(
            (signals[0] - 0.9).abs() < 1e-4,
            "the pool feed steps around a hand-set slot, got {}",
            signals[0]
        );
        set_post_shader_manual(Vec::new());
        set_post_shader_routes(Vec::new());
    }
}

#[cfg(test)]
mod post_shader_program_tests {
    use super::*;
    use panel::shader::ProgramCtx;

    const SOURCE: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";

    /// The screen shader's images resolve from wherever its text came
    /// from, so the resolve hands both back together rather than leaving
    /// each driver to work the origin out again.
    #[test]
    fn the_post_program_carries_where_its_source_came_from() {
        // The cache alone, so a test run leaves no agreement behind in the
        // settings file.
        settings::note_approved(&panel::shader::fingerprint(SOURCE));

        // Inline, the way a shader arrives inside a look: detached, since
        // nothing on this machine holds what it might declare.
        let inline = PostShaderConfig {
            source: SOURCE.to_string(),
            ..PostShaderConfig::default()
        };
        assert_eq!(
            post_shader_program(&inline).expect("inline resolves"),
            Some((SOURCE.to_string(), ProgramCtx::detached()))
        );

        // A name wins over the inline copy, and the images then come out of
        // the pool entry.
        settings::note_shader_pool(vec![settings::NamedShader {
            name: "Grain".to_string(),
            source: SOURCE.to_string(),
            path: None,
            assets: Vec::new(),
        }]);
        let named = PostShaderConfig {
            name: Some("Grain".to_string()),
            source: "// the config's own".to_string(),
            ..PostShaderConfig::default()
        };
        assert_eq!(
            post_shader_program(&named).expect("the pool resolves"),
            Some((SOURCE.to_string(), ProgramCtx::named("Grain")))
        );
        // And a name that matches no entry is nothing to run, images or not.
        settings::note_shader_pool(Vec::new());
        assert_eq!(
            post_shader_program(&named).expect("a miss isn't an error"),
            None
        );

        // A file, whose folder is where its images are looked for.
        let dir = std::env::temp_dir().join("rox-post-shader-origin");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("post.wgsl");
        std::fs::write(&path, SOURCE).expect("write");
        let file = PostShaderConfig {
            path: Some(path.clone()),
            ..PostShaderConfig::default()
        };
        assert_eq!(
            post_shader_program(&file).expect("the file resolves"),
            Some((SOURCE.to_string(), ProgramCtx::file(&path)))
        );
        std::fs::remove_dir_all(&dir).ok();

        settings::forget_approved(&panel::shader::fingerprint(SOURCE));
    }
}
