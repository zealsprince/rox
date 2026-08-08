//! The main window: an in-window menubar over the dock area. gpui only
//! surfaces `set_menus` in the macOS system bar, so the bar is drawn
//! in-window to behave the same on every platform. The dock, tabs, splits,
//! and resize come from gpui-component per ADR 7; duplicate and pop-out
//! live on the panels themselves. Playback UI is the transport panels in
//! the bottom dock; the PCM tap that feeds the audio views is drained by
//! the player's own pump task, so nothing here has to keep rendering for
//! playback's sake.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use gpui::{
    actions, deferred, div, overlay_phase, prelude::*, px, svg, AnyElement, AnyWindowHandle, App,
    Axis, Context, DismissEvent, Div, Entity, ExternalPaths, FocusHandle, Focusable as _,
    FontFeatures, Global, KeyBinding, KeyDownEvent, MouseButton, PathPromptOptions, SharedString,
    Subscription, Task, WeakEntity, Window, WindowBounds,
};
use rox_dock::{
    register_panel, DockArea, DockAreaState, DockEvent, DockItem, Panel as _, PanelInfo, PanelView,
    StackPanel, TabPanel, ToggleZoom,
};

use gpui::rgba;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::PopupMenu;
use gpui_component::Icon;

use crate::composite;
use crate::integrations::media_controls::MediaSession;
use crate::integrations::tray;
use crate::panel_catalog::{self as catalog, PanelDef, PanelPlacement, PanelSection};
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
    self, LastTrack, LayoutEdit, LayoutSize, NamedLayout, QueueState, QueuedTrack, Settings,
    WindowState, WorkspaceBundle,
};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, TabHosts};
use rox_panel_api::query::shared_query::SharedQuery;
use rox_panel_api::track_ui::track_drag::PlayDrag;
use rox_panels::art::{ArtConfig, ArtPanel};
use rox_panels::artist_grid::{ArtistGridConfig, ArtistGridPanel};
use rox_panels::biography::BiographyPanel;
use rox_panels::cover::CoverArtPanel;
use rox_panels::drag_anchor::DragAnchorPanel;
use rox_panels::eq_widget::EqWidgetPanel;
use rox_panels::favourite::FavouritePanel;
use rox_panels::filter::{FilterConfig, FilterPanel};
use rox_panels::folder_tree::FolderTreePanel;
use rox_panels::genre_grid::{GenreGridConfig, GenreGridPanel};
use rox_panels::grid::{GridConfig, GridPanel};
use rox_panels::history::HistoryPanel;
use rox_panels::library::{LibraryConfig, LibraryPanel};
use rox_panels::lyrics::{LyricsPanel, StampLine};
use rox_panels::metadata::MetadataPanel;
use rox_panels::output::OutputPanel;
use rox_panels::particles::ParticlesPanel;
use rox_panels::playlists::PlaylistsPanel;
use rox_panels::queue::QueuePanel;
use rox_panels::rating::RatingPanel;
use rox_panels::search::{SearchConfig, SearchPanel};
use rox_panels::shader::ShaderPanel;
use rox_panels::spacer::SpacerPanel;
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

// The registry of open workspace windows lives in rox-panel-api now, so
// the app-level lookups panels reach for can answer without the Workspace
// type. It holds the entity type-erased; the typed wrappers below downcast
// it back. front_workspace answers with the handle and the shared state,
// which is all its callers (the tray, the taskbar, the tasks/EQ/signals/
// console windows, the single-instance guard) ever wanted.
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
    // very window this renegotiates, and a window can't be updated while it
    // is already on the update stack - the re-entrant update errs and the
    // window silently keeps its old chrome until restart.
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

/// What the last post shader compile said, for the Shader settings page's
/// readout: None is a clean compile (or nothing installed). Shared across
/// windows because they all wear the same file; the last one to compile
/// wins, which for one file is the same message.
static POST_SHADER_ERROR: RwLock<Option<String>> = RwLock::new(None);

/// The last post shader compile error, read live by the settings window.
pub(crate) fn post_shader_error() -> Option<String> {
    POST_SHADER_ERROR.read().unwrap().clone()
}

/// The shader switch as it currently stands, mirroring the settings file.
/// A live static like `hide_menubar`'s, because the Appearance toggle, the
/// menu row, and the hotkey all flip it and all have to show one state.
static POST_SHADER_ON: AtomicBool = AtomicBool::new(false);

pub(crate) fn post_shader_on() -> bool {
    POST_SHADER_ON.load(Ordering::Relaxed)
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

/// The slot names the screen shader's source declares, from its `// @slot
/// n: name` comments. Read where the file is, published here so the
/// settings window's route editor can name slots without opening the file
/// on every render.
static POST_SHADER_LABELS: RwLock<Vec<Option<String>>> = RwLock::new(Vec::new());

pub(crate) fn post_shader_slot_labels() -> Vec<Option<String>> {
    POST_SHADER_LABELS.read().unwrap().clone()
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

/// The child windows currently wearing the shader under the all-windows
/// option, and the composed source they wear. Workspace windows never
/// appear here; they keep their own [`PostShaderDriver`]s.
#[derive(Default)]
struct ShadedChildren {
    source: Option<String>,
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

/// Reapply the configured post shader everywhere. The Shader settings page's
/// controls, the toggle action, the confirm dialog's revert, and the hot
/// reload all land here; each workspace window re-reads the file and
/// compiles it fresh, then the child windows follow when the all-windows
/// option is on. Deferred like the decorations apply, so an in-window
/// trigger can't re-enter its own update.
pub(crate) fn apply_post_shader(cx: &mut App) {
    cx.defer(|cx| {
        let config = Settings::load().post_shader;
        POST_SHADER_ON.store(config.enabled, Ordering::Relaxed);
        let open: Vec<(AnyWindowHandle, Entity<Workspace>)> = cx
            .default_global::<WorkspaceWindows>()
            .open
            .iter()
            .filter_map(|w| Some((w.handle, typed_workspace(&w.workspace)?)))
            .collect();
        for (handle, workspace) in open {
            handle
                .update(cx, |_, window, cx| {
                    workspace.update(cx, |workspace, _| workspace.apply_post_shader(window));
                })
                .ok();
        }
        // The child pass: cache the source and shade every eligible window,
        // or strip the ones shaded before. Read errors fall through as None
        // here; the workspace pass above already surfaced them.
        let source = (config.enabled && config.all_windows)
            .then(|| {
                config
                    .path
                    .as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
            })
            .flatten();
        let previous = std::mem::take(&mut cx.default_global::<ShadedChildren>().windows);
        cx.default_global::<ShadedChildren>().source = source.clone();
        if source.is_some() {
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
        // shaded windows land in the same frame.
        for window in cx.windows() {
            window.update(cx, |_, window, _| window.refresh()).ok();
        }
    });
}

/// Shade every eligible window that isn't wearing the source yet: child
/// windows opened after the apply join through the workspace's periodic
/// sweep. Workspace windows keep their own drivers, and the confirm
/// dialog is always skipped.
fn sweep_shaded_children(cx: &mut App) {
    let Some(source) = cx.default_global::<ShadedChildren>().source.clone() else {
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
    for handle in cx.windows() {
        if workspaces.contains(&handle) || Some(handle) == confirm || shaded.contains(&handle) {
            continue;
        }
        let installed = handle
            .update(cx, |_, window, _| {
                match window.register_user_shader(&source) {
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
/// rides the workspace loop.
fn push_child_signals(signals: [f32; 16], cx: &mut App) {
    let shaded = cx.default_global::<ShadedChildren>().windows.clone();
    for handle in shaded {
        let alive = handle
            .update(cx, |root, window, cx| {
                // Zeroed meta until a later unit assigns the eight slots.
                window.set_post_signals(signals, [0.0; 8]);
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
/// With none, the pool feeds the slots in its own order - the behaviour
/// from before the routes existed, kept because a setup tuned against it
/// would otherwise go dark on upgrade. The first route someone adds takes
/// over the whole feed, which is the only reading that doesn't have two
/// things writing the same slot.
fn post_shader_signals(hub: &SignalHub) -> [f32; panel::shader::SLOTS] {
    let routes = POST_SHADER_ROUTES.read().unwrap();
    if routes.is_empty() {
        let mut signals = [0.0f32; panel::shader::SLOTS];
        for (slot, signal) in hub.pool().iter().take(signals.len()).enumerate() {
            signals[slot] = hub.value(signal.id).unwrap_or(0.0);
        }
        return signals;
    }
    let mut targets = panel::shader::SlotTargets::default();
    rox_panel_api::signal_ui::apply_routes(&routes, hub, &mut targets);
    targets.slots
}

/// The per-window side of the post shader (the Shader settings page's Screen
/// Shader section): which file this window wears and its stamp, for the
/// render loop's hot reload.
struct PostShaderDriver {
    path: PathBuf,
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
            // answer while the app is only an icon. Windows' SMTC is bound to
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
    // Each window's service speaks for its own player, so the survivor
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
/// under a reflex. With tray off it's a no-op there - use the menu or Cmd+Q.
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
    state.player.update(cx, |player, cx| match mode {
        rox_library::open_files::LaunchMode::Play => player.play(paths, cx),
        rox_library::open_files::LaunchMode::Enqueue => player.enqueue(paths, cx),
    });
}

/// Apply a named workspace to the frontmost workspace window from app
/// level, the same path the settings window's Apply takes. The welcome
/// window's quick-start tiles land here: they live in their own OS window,
/// with no workspace of their own to call into.
pub(crate) fn apply_workspace_to_front(name: &str, cx: &mut App) {
    let found = cx
        .default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find_map(|w| Some((w.handle, typed_workspace(&w.workspace)?)));
    if let Some((handle, ws)) = found {
        let name = name.to_string();
        let _ = handle.update(cx, |_, window, cx| {
            ws.update(cx, |ws, cx| ws.apply_workspace(&name, window, cx));
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

/// The workspace hosting `window`, when it is a workspace window (not a
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
/// the way [`rox_panel_api::query::shared_query::search_flyout`] builds its Search flyout -
/// a hand-built menu entity behind a submenu item - so it works from every
/// host of the panel menu, the content context menus included. Leads with a
/// divider so it reads as its own band rather than the tail of whatever
/// content section sits above it; the separator is a no-op when Add Panel
/// would be the menu's first item. A popped-out panel (no group) or a window
/// with no workspace behind it gets nothing.
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
    let Some(workspace) = cx
        .default_global::<WorkspaceWindows>()
        .open
        .iter()
        .find(|w| w.handle == handle)
        .and_then(|w| typed_workspace(&w.workspace))
        .map(|ws| ws.downgrade())
    else {
        return menu;
    };
    let submenu = PopupMenu::build(window, cx, move |mut menu, window, cx| {
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
                        label,
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
        gpui_component::menu::PopupMenuItem::submenu("Add Panel", submenu)
            .icon(Icon::default().path(icons::PLUS)),
    )
}

/// One Add Panel row: build the def's panel against the workspace's state
/// and land it as a tab of the clicked group.
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
                .child(div().flex_1().child(def.label))
                .child(
                    svg()
                        .path(icons::AUDIO_WAVEFORM)
                        .size_3()
                        .text_color(palette::text_faint()),
                )
        })
    } else {
        gpui_component::menu::PopupMenuItem::new(def.label)
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

// The three playback actions panels bind against live in rox-panel-api, so
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

/// Bindings match key contexts along the focus path, so this scope holds
/// anywhere inside a workspace window except while the library search box
/// is focused: there space and arrows keep typing into the query. Bindings
/// win over key listeners, the exclusion is what hands the keys back.
const PLAYBACK_KEY_SCOPE: Option<&str> = Some("Workspace && !SearchInput");

/// App-level key bindings; call once at startup.
pub fn init(cx: &mut App) {
    // Quit binds unscoped so it fires in every window, popped-out panels
    // and the search box included. The macOS system menu is not set, so
    // Cmd+Q only exists through this binding.
    let quit_keys = if cfg!(target_os = "macos") {
        "cmd-q"
    } else {
        "alt-f4"
    };
    // Preferences shortcut follows the platform: Cmd+, on macOS, Ctrl+,
    // elsewhere. Ctrl+I is a second binding on both. These carry modifiers,
    // so they stay unscoped past the search box without stealing typing.
    let settings_keys = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    // The quick-play modal answers both the palette chord and the find
    // chord; either habit lands in the same search.
    let (quick_play_p, quick_play_f) = if cfg!(target_os = "macos") {
        ("cmd-p", "cmd-f")
    } else {
        ("ctrl-p", "ctrl-f")
    };
    // Stats takes the shifted S so it stays clear of the settings and
    // quick-play chords.
    let stats_keys = if cfg!(target_os = "macos") {
        "cmd-shift-s"
    } else {
        "ctrl-shift-s"
    };
    // Jump to the search box, the browser's address-bar chord. Modified, so
    // it stays out of the way of typing in the box itself.
    let focus_search_keys = if cfg!(target_os = "macos") {
        "cmd-l"
    } else {
        "ctrl-l"
    };
    // Text-zoom chords, the browser's font shortcuts: Cmd/Ctrl with the +/-
    // keys steps the app font size up and down, and 0 snaps it back to the
    // stock size. The `=` key doubles for `+` so it works without reaching
    // for shift, the way every browser binds it. Bound unscoped so they carry
    // across every window - settings, about, popped-out panels - like Quit;
    // the modifiers keep them out of the search boxes' typing.
    let (zoom_in, zoom_in_shift, zoom_out, zoom_reset) = if cfg!(target_os = "macos") {
        ("cmd-=", "cmd-+", "cmd--", "cmd-0")
    } else {
        ("ctrl-=", "ctrl-+", "ctrl--", "ctrl-0")
    };
    // Close-window chord: Cmd+W on macOS, Ctrl+W elsewhere. Unscoped like
    // Quit so it fires in every window; the handler decides what a close
    // means per window.
    let close_window_keys = if cfg!(target_os = "macos") {
        "cmd-w"
    } else {
        "ctrl-w"
    };
    // The screen shader kill switch, X as in fx. Unscoped on purpose: a
    // hostile shader can bury every control this key would be reached by,
    // so it has to fire from whichever window still has focus.
    let post_shader_keys = if cfg!(target_os = "macos") {
        "cmd-shift-x"
    } else {
        "ctrl-shift-x"
    };
    cx.bind_keys([
        KeyBinding::new("space", TogglePlayback, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("left", SeekBackward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new("right", SeekForward, PLAYBACK_KEY_SCOPE),
        KeyBinding::new(settings_keys, OpenSettings, Some("Workspace")),
        KeyBinding::new("ctrl-i", OpenSettings, Some("Workspace")),
        KeyBinding::new(stats_keys, OpenStats, Some("Workspace")),
        KeyBinding::new(quick_play_p, OpenQuickPlay, Some("Workspace")),
        KeyBinding::new(quick_play_f, OpenQuickPlay, Some("Workspace")),
        KeyBinding::new(focus_search_keys, FocusSearch, Some("Workspace")),
        KeyBinding::new(zoom_in, IncreaseFontSize, None),
        KeyBinding::new(zoom_in_shift, IncreaseFontSize, None),
        KeyBinding::new(zoom_out, DecreaseFontSize, None),
        KeyBinding::new(zoom_reset, ResetFontSize, None),
        // Fullscreens the last-clicked panel group over the whole dock
        // area; the same chord or a plain escape backs out. Shift keeps
        // it off the search boxes' bare-escape ladder. This is the dock's
        // own action, so the zoom controls in every panel's menus render
        // the chord next to "Zoom In".
        KeyBinding::new("shift-escape", ToggleZoom, Some("Workspace")),
        // Stamp the current time onto a lyric line, live only inside the
        // lyrics editor (the LyricsEdit context). Shift+Enter is the same
        // chord on every platform and the input leaves it unbound, unlike
        // plain and secondary Enter which type a newline.
        KeyBinding::new("shift-enter", StampLine, Some("LyricsEdit")),
        KeyBinding::new(post_shader_keys, TogglePostShader, None),
        KeyBinding::new(close_window_keys, CloseWindow, None),
        KeyBinding::new(quit_keys, Quit, None),
    ]);
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
    // has focus dispatches, and the apply reaches them all.
    cx.on_action(|_: &TogglePostShader, cx| toggle_post_shader(cx));
}

/// Nudge the app font size by `delta` px from where it stands.
fn nudge_font_size(delta: f32, cx: &mut App) {
    set_font_size(palette::app_font_size() + delta, cx);
}

/// Set the app font size and persist it. The live setter clamps to the
/// palette's range and repaints every window; the settings write keeps the
/// new size across launches, the same pair the settings slider drives. A
/// user-level readability choice, so it lives on `Settings` directly and
/// rides out workspace switches, like the theme pick.
fn set_font_size(size: f32, cx: &mut App) {
    let size = size.clamp(palette::FONT_SIZE_MIN, palette::FONT_SIZE_MAX);
    palette::set_app_font_size(size, cx);
    Settings::update(move |s| s.app_font_size = size);
    // The setter's repaint loop reaches every window but the one dispatching
    // this action: we're mid-update inside it, so its re-entrant refresh is
    // dropped and the resize would sit until the next input event. Defer a
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
    // A panel whose config rides the layout dump.
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
    // Filter carries a window at build so its quick-search box can spin up
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
    configured!("rating", RatingPanel);
    configured!("favourite", FavouritePanel);
    configured_windowed!("playlists", PlaylistsPanel);
    // The composition hosts rebuild their children through this same
    // registry, and carry the workspace handle so their slot menus can
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
    // The artist wall carries a search box too, so it takes the window like
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
    configured!("waveform", WaveformPanel);
    configured!("vu meter", VuPanel);
    // Registered whether or not experimental features are on: the flag
    // gates the panel menus, not a layout that already holds one.
    configured!("particles", ParticlesPanel);
    configured!("shader", ShaderPanel);
    configured!("drag anchor", DragAnchorPanel);
    configured!("spacer", SpacerPanel);
    configured!("theme toggle", ThemeTogglePanel);
    // These two drive the workspace back, so their builders carry its
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
    /// Open the shared signal pool, the window every panel's routes ride.
    OpenSignals,
    OpenWelcome,
    OpenAbout,
    ToggleMenubar,
    ToggleDecorations,
    /// Flip song theming: the playing track's art tinting the palette.
    ToggleArtTheming,
    /// Flip the screen shader, the hotkey's menu twin. Never prompts.
    TogglePostShader,
    /// Pick a workspace file and add it to the collection.
    ImportWorkspace,
    /// Open a catalog panel with its default config, landing where its
    /// placement says. One action for every panel the catalog carries.
    OpenPanel(&'static PanelDef),
    ToggleQuitToTray,
    CloseWindow,
    Quit,
}

#[derive(Clone, Copy)]
pub(crate) struct MenuItem {
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) action: MenuAction,
}

/// A native-menu pick, carried as a string so one action covers the whole
/// menu tree. The macOS system bar dispatches actions rather than calling
/// our handlers, and only a registered `Action` can ride through it, so
/// every row that isn't already a bound action (Play, Settings, Stats,
/// Quit) encodes itself here and [`run_menu_command`] decodes it back.
/// See [`MenuAction::command_id`] for the encoding. `no_json` because this
/// is only ever built in code, never named in a keymap file, which is what
/// the JSON path is for - it also keeps schemars out of our dependencies.
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
            // These ride their own actions, for the shortcut.
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
            MenuAction::ToggleMenubar => "toggle-menubar".into(),
            MenuAction::ToggleDecorations => "toggle-decorations".into(),
            MenuAction::ToggleArtTheming => "toggle-art-theming".into(),
            MenuAction::TogglePostShader => "toggle-post-shader".into(),
            MenuAction::ImportWorkspace => "import-workspace".into(),
            MenuAction::ToggleQuitToTray => "toggle-quit-to-tray".into(),
            MenuAction::CloseWindow => "close-window".into(),
            MenuAction::OpenPanel(def) => format!("panel:{}", def.label),
        };
        Some(id)
    }

    /// The inverse of [`MenuAction::command_id`]. A panel resolves through
    /// the catalog, so a row for a panel that has since been gated off
    /// (experimental) decodes to None and the pick does nothing.
    fn from_command_id(id: &str) -> Option<MenuAction> {
        if let Some(label) = id.strip_prefix("panel:") {
            return catalog::sections()
                .flat_map(|section| section.panels.iter())
                .find(|def| def.label == label)
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
/// a menu pick can land while that window is mid-update (its own render
/// dispatched the action), and updating a window already on the update stack
/// is refused - the same reason [`close_active_window`] defers.
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
/// workspace and layout rows carry a name, so they route to the same
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
        // "panel:<label>" and anything else that carries a colon.
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
    pub(crate) label: &'static str,
    pub(crate) entries: &'static [MenuEntry],
}

pub(crate) const MENUS: &[Menu] = &[
    Menu {
        label: "Application",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "Settings",
                icon: icons::SETTINGS,
                action: MenuAction::OpenSettings,
            }),
            // The two instruments: windows you work while the music plays,
            // rather than preferences you set and close.
            MenuEntry::Section("Tuning"),
            MenuEntry::Item(MenuItem {
                label: "Equalizer",
                icon: icons::AUDIO_LINES,
                action: MenuAction::OpenEqualizer,
            }),
            MenuEntry::Item(MenuItem {
                label: "Signals",
                icon: icons::AUDIO_WAVEFORM,
                action: MenuAction::OpenSignals,
            }),
            MenuEntry::Section("Library"),
            MenuEntry::Item(MenuItem {
                label: "Stats",
                icon: icons::CHART_PIE,
                action: MenuAction::OpenStats,
            }),
            MenuEntry::Item(MenuItem {
                label: "Tasks",
                icon: icons::CLOCK,
                action: MenuAction::OpenTasks,
            }),
            MenuEntry::Section("App"),
            MenuEntry::Item(MenuItem {
                label: "Console",
                icon: icons::FILE_TEXT,
                action: MenuAction::OpenConsole,
            }),
            MenuEntry::Item(MenuItem {
                label: "Welcome",
                icon: icons::INFO,
                action: MenuAction::OpenWelcome,
            }),
            MenuEntry::Item(MenuItem {
                label: "About",
                icon: icons::LOGO,
                action: MenuAction::OpenAbout,
            }),
            MenuEntry::Section("Session"),
            MenuEntry::Item(MenuItem {
                label: "Exit",
                icon: icons::CLOSE,
                action: MenuAction::Quit,
            }),
        ],
    },
    Menu {
        label: "Playback",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "Play",
                icon: icons::PLAY,
                action: MenuAction::TogglePlayback,
            }),
            MenuEntry::Item(MenuItem {
                label: "Stop",
                icon: icons::STOP,
                action: MenuAction::Stop,
            }),
            MenuEntry::Section("Track"),
            MenuEntry::Item(MenuItem {
                label: "Next",
                icon: icons::SKIP_FORWARD,
                action: MenuAction::Next,
            }),
            MenuEntry::Item(MenuItem {
                label: "Previous",
                icon: icons::SKIP_BACK,
                action: MenuAction::Previous,
            }),
        ],
    },
    Menu {
        label: "Window",
        entries: &[
            MenuEntry::Item(MenuItem {
                label: "New Window",
                icon: icons::PLUS,
                action: MenuAction::NewWindow,
            }),
            MenuEntry::Item(MenuItem {
                label: "Empty Window",
                icon: icons::SQUARE_DASHED,
                action: MenuAction::EmptyWindow,
            }),
            MenuEntry::Section("Interface"),
            MenuEntry::Item(MenuItem {
                label: "Hide Menubar",
                icon: icons::EYE,
                action: MenuAction::ToggleMenubar,
            }),
            MenuEntry::Item(MenuItem {
                label: "OS Decorations",
                icon: icons::APP_WINDOW,
                action: MenuAction::ToggleDecorations,
            }),
            MenuEntry::Item(MenuItem {
                label: "Song Theming",
                icon: icons::DISC,
                action: MenuAction::ToggleArtTheming,
            }),
            MenuEntry::Item(MenuItem {
                label: "Screen Shader",
                icon: icons::BLEND,
                action: MenuAction::TogglePostShader,
            }),
            MenuEntry::Section("Session"),
            MenuEntry::Item(MenuItem {
                label: "Remain in Tray",
                icon: icons::MINIMIZE,
                action: MenuAction::ToggleQuitToTray,
            }),
            MenuEntry::Item(MenuItem {
                // Closes this window, unlike Application's Exit which quits.
                label: "Close",
                icon: icons::CLOSE,
                action: MenuAction::CloseWindow,
            }),
        ],
    },
    Menu {
        label: "Workspace",
        entries: &[
            MenuEntry::WorkspacesSubmenu {
                label: "Apply Workspace",
                icon: icons::GALLERY,
                target: WorkspaceTarget::Apply,
                with_new: false,
            },
            MenuEntry::WorkspacesSubmenu {
                label: "Save Workspace",
                icon: icons::DOWNLOAD,
                target: WorkspaceTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::Item(MenuItem {
                label: "Import Workspace...",
                icon: icons::UPLOAD,
                action: MenuAction::ImportWorkspace,
            }),
            MenuEntry::Section("Layouts"),
            MenuEntry::LayoutsSubmenu {
                label: "New Window from Layout",
                icon: icons::LAYOUT_DASHBOARD,
                target: LayoutTarget::NewWindow,
                with_new: false,
            },
            MenuEntry::LayoutsSubmenu {
                label: "Save Layout",
                icon: icons::UPLOAD,
                target: LayoutTarget::Overwrite,
                with_new: true,
            },
            MenuEntry::LayoutsSubmenu {
                label: "Apply Layout",
                icon: icons::DOWNLOAD,
                target: LayoutTarget::Apply,
                with_new: false,
            },
        ],
    },
    Menu {
        label: "Panels",
        entries: &[
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
        MenuAction::TogglePlayback if is_playing => ("Pause", icons::PAUSE),
        _ => (item.label, item.icon),
    }
}

/// Whether a dropdown row trails the signal glyph: a catalog row for a
/// panel whose settings carry knobs the shared pool can drive. Read by
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
    ConfirmApplyWorkspace(String),
    /// Taking a pinned panel out of the layout. Carries what to close so the
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
    /// A mouse button is held down somewhere in the window. Alt+drag is
    /// the compositor's window move/resize, so an alt-revealed menubar
    /// stays hidden while a button is down: the overlay must not sit in
    /// front of the drag. Tracked in the capture phase so an occluding
    /// child can't hide the press.
    pointer_down: bool,
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
    /// The tab panel the layout starts with. Panels-menu panels land here
    /// while it is still showing.
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
    layout_error: Option<&'static str>,
    /// The mini-player button's two presets, by name. Cached off the settings
    /// file so the menubar never reads disk per frame; the settings window
    /// pushes changes back through [`Workspace::set_mini_roles`]. The button
    /// hides unless a mini preset is set.
    primary_layout: Option<String>,
    mini_layout: Option<String>,
    /// The named preset this window is on, mirrored to settings on every
    /// apply. Which side of the mini toggle shows falls out of comparing it to
    /// `mini_layout`; a workspace save captures into it. None is an unnamed
    /// arrangement.
    active_layout: Option<String>,
    /// The layout save/apply dialog while it is up; dropped on close.
    layout_dialog: Option<LayoutDialog>,
    /// Submits the save dialog's name field on Enter.
    _layout_input: Option<Subscription>,
    /// The quick-play modal while it is up; dropped on dismiss.
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
    titled_track: Option<PathBuf>,
    _layout_changed: Subscription,
    /// The player pump notifies every tick while a session runs; the
    /// title refresh rides it and bails on the path compare.
    _player_changed: Subscription,
    /// The menubar's right side shows the catalog status, so library
    /// updates must repaint the workspace.
    _library_changed: Subscription,
    /// A landed listen bumps its track's play count in the shared
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
    /// window holds it rather than owns it - a close hands it to a surviving
    /// window or to the tray, and it only dies when the app does.
    media: Option<Entity<MediaSession>>,
    /// The whole-window post shader this window wears, None while the
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
/// stack rides along for [`Workspace::add_bottom`] to attach on first use,
/// same as a restored layout with no row.
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
/// root stack, the first tab group (where add_center prefers to land), and
/// the last horizontal split (the transport row add_bottom appends to).
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
/// was open. Only a reopen carries one; every other open starts from nothing.
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
        // the hold, so the playing player, library, and art carry straight
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
        // The mini-player roles ride in the struct so the menubar never
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
                let mut paths = Vec::with_capacity(q.entries.len());
                let mut explicit = Vec::with_capacity(q.entries.len());
                let mut cursor = 0;
                for (i, entry) in q.entries.iter().enumerate() {
                    let path = library
                        .paths_for(&[entry.id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    if let Some(path) = path {
                        if i <= q.cursor {
                            cursor = paths.len();
                        }
                        paths.push(path);
                        explicit.push(entry.explicit);
                    }
                }
                (!paths.is_empty()).then_some((paths, explicit, cursor, q.position_secs))
            });
            if let Some((paths, explicit, cursor, position_secs)) = queue {
                state.player.update(cx, |player, cx| {
                    player.restore_queue(paths, explicit, cursor, position_secs, cx)
                });
            } else if let Some(last) = settings.session.last_track {
                let path = state
                    .library
                    .read(cx)
                    .paths_for(&[last.id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                if let Some(path) = path {
                    state.player.update(cx, |player, cx| {
                        player.restore(path, last.position_secs, cx)
                    });
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
        // A dump that exists but will not restore, or a preset whose name no
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
                    "This window's layout preset couldn't be restored, so it starts empty."
                }
                _ => "The saved layout couldn't be restored, so this window starts empty.",
            };
            log::warn!("workspace: {message}");
            message
        });

        let (center, stack, center_tabs, bottom_stack) = match restored {
            Some(item) => {
                let (stack, tabs, bottom) = layout_views(&item);
                // The preferred add targets may not survive a rearranged
                // layout; fresh detached entities take their place, and the
                // add paths attach them back into the tree on first use.
                let tabs = tabs.unwrap_or_else(|| tabs_item(Vec::new(), &weak_dock, window, cx).1);
                let bottom = bottom
                    .unwrap_or_else(|| cx.new(|cx| StackPanel::new(Axis::Horizontal, window, cx)));
                (item, stack, tabs, bottom)
            }
            // Everything without a restorable dump starts empty: the blank
            // window and the first run by design, a broken dump because an
            // empty window that says why beats a default arrangement nobody
            // arranged.
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
            // The shader's frame loop lives in render and sustains itself
            // through frame requests, but only render can start it. Nothing
            // else re-renders the workspace on a resume, so a parked shader
            // would sit frozen until a track change; while one is worn, the
            // pump's tick is what re-arms it.
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

        // The primary window owns the OS media service: a session the tray
        // hands back when it kept one alive, a fresh registration otherwise.
        // Everything past that point is the session's own business - it
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
            pointer_down: false,
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
        // restart wears it without a trip through the settings window.
        this.apply_post_shader(window);
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
    /// The read and compile happen here, synchronously: a failure logs, lands
    /// its message in the shared readout, and leaves whatever compiled last
    /// still running, so a broken edit never blanks the effect.
    fn apply_post_shader(&mut self, window: &mut Window) {
        let config = Settings::load().post_shader;
        // Keep the live switch and the route feed in step; the startup path
        // lands here before any app-level apply has run.
        POST_SHADER_ON.store(config.enabled, Ordering::Relaxed);
        set_post_shader_routes(config.routes);
        let (true, Some(path)) = (config.enabled, config.path) else {
            if self.post_shader.take().is_some_and(|driver| driver.active) {
                window.set_post_shader(None);
            }
            *POST_SHADER_ERROR.write().unwrap() = None;
            *POST_SHADER_LABELS.write().unwrap() = Vec::new();
            return;
        };
        let mut driver = PostShaderDriver {
            stamp: settings::file_stamp(&path),
            checked: Instant::now(),
            active: self
                .post_shader
                .as_ref()
                .is_some_and(|driver| driver.active),
            path,
        };
        let result = std::fs::read_to_string(&driver.path)
            .map_err(|e| format!("reading {}: {e}", driver.path.display()))
            .and_then(|source| {
                // The slot names travel with the source, so the settings
                // window's route editor names them the way a panel's does.
                *POST_SHADER_LABELS.write().unwrap() = panel::shader::slot_labels(&source);
                window.register_user_shader(&source)
            });
        match result {
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

    /// The screen shader's frame loop, run from render: re-read the file
    /// when its stamp moves (one stat a second, the pump cadence render
    /// already rides), and while audio moves, feed the pool's signals into
    /// the shader's slots (slot i takes pool signal i) and ask for the next
    /// frame. A silent hub stops the feed, which freezes the shader clock
    /// and parks the frame requests, the same self-parking discipline the
    /// visualizer panels keep.
    fn drive_post_shader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(driver) = &mut self.post_shader else {
            return;
        };
        // The oldest open workspace speaks for the app-wide concerns: the
        // child sweeps and their signal feed run once, not per workspace.
        // By open serial, not the list's head - the registry reorders on
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
            if settings::file_stamp(&driver.path) != driver.stamp {
                // Through the app-level apply, so every shaded window
                // follows the edit rather than just this one. Never
                // prompts: the hot reload is the authoring loop.
                apply_post_shader(cx);
            } else if primary {
                // Catch child windows opened since the last apply.
                cx.defer(sweep_shaded_children);
            }
        }
        let Some(driver) = &self.post_shader else {
            return;
        };
        if !driver.active {
            return;
        }
        let hub = &self.state.signals;
        // Tick before the live check. Live only goes true once a tick has
        // seen the feed move, and on a workspace without a particles panel
        // or the signals window this loop is the hub's only ticker, so
        // checking first parked the shader for good: no signals, frozen
        // clock. The tick itself is cheap and TICK_MIN-deduped.
        let player = self.state.player.read(cx);
        hub.tick(&player.feed(), player.playing_entry());
        if !hub.live() {
            return;
        }
        let signals = post_shader_signals(hub);
        // Zeroed meta until a later unit assigns the eight slots.
        window.set_post_signals(signals, [0.0; 8]);
        window.request_animation_frame();
        if primary {
            // Deferred: a window can't update its siblings from inside its
            // own render.
            cx.defer(move |cx| push_child_signals(signals, cx));
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
        // the swap, so switching away keeps its unsaved tweaks. Synchronous
        // on purpose: the debounced save below would otherwise be the only
        // writer, and it dumps the incoming layout, not this one.
        self.stash_active_edits(window, cx);
        // The registry's builders capture one workspace's entities;
        // re-register so the rebuild lands on this one even after
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
    /// hint says why it is empty, rather than a stand-in arrangement nobody
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
        self.layout_error =
            Some("The workspace's layout couldn't be restored, so this window starts empty.");
        log::warn!("workspace: applied workspace has no restorable layout, starting empty");
        // The empty build has no name of its own.
        self.set_active_layout(None);
        self.save_layout_soon(window, cx);
    }

    /// Record the named preset the window is now on and mirror it to settings,
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
    /// f32 sizes and panel configs to f64, so a raw dump carries 17-digit
    /// float tails; [`denoise_f32`] snaps them back to clean numbers.
    fn dock_dump(&self, cx: &App) -> serde_json::Result<serde_json::Value> {
        let mut value = serde_json::to_value(self.dock.read(cx).dump(cx))?;
        denoise_f32(&mut value);
        Ok(value)
    }

    /// Fold this window's live dock into the active layout's working copy,
    /// the unsaved-tweaks store a later switch reads back. A window on an
    /// unnamed arrangement (the default build, a one-off import) has no name
    /// to key on, so this no-ops; its live dock rides in `settings.look.layout`
    /// for the launch restore instead.
    fn stash_active_edits(&self, window: &Window, cx: &mut Context<Self>) {
        let Some(name) = self.active_layout.clone() else {
            return;
        };
        let Ok(dump) = self.dock_dump(cx) else {
            return;
        };
        // The current window size rides along, live off the window rather than
        // the debounced `settings.windows.main`, so a resize made just before the
        // switch comes back with the layout.
        let size = Some(window_size(window));
        Settings::update(move |s| {
            s.look.layout_edits.insert(name, LayoutEdit { dump, size });
        });
    }

    /// Apply a saved preset by name. Returns false when
    /// no preset carries the name or its dump is one [`apply_layout`]
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
                // The working copy's own size wins when it carries one; a copy
                // from before sizes rode along keeps the preset's.
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
        // size, or the preset's); neither carrying one leaves the window as is.
        // Except in a macOS fullscreen Space, where AppKit owns the frame and
        // a resize is dropped or fights the Space: swap the layout, leave the
        // frame alone. The mini toggle exits fullscreen before applying, so
        // its resize still lands. A `--window-size` launch also pins the
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
        // stops pumping frames for a window that is idle and not focused.
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
    /// carries no layout. The empty launcher's way to start from a vendored
    /// look; a blank window has nothing to replace, so it acts straight off
    /// the click with no confirm. The settings window's apply lands here
    /// too, so both entry points share one flow.
    pub(crate) fn apply_workspace(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        crate::workspaces::apply_look(&bundle, cx);
        // The look's signal pool replaces the live one the same wholesale
        // way; apply_look already persisted it into settings.
        self.state.signals.set_pool(bundle.signals.clone());
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
    /// active layout rather than a separate flag, so it is always in step with
    /// what is actually showing.
    pub(crate) fn on_mini(&self) -> bool {
        self.mini_layout.is_some() && self.active_layout == self.mini_layout
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
            WorkspaceTarget::Apply => LayoutDialog::ConfirmApplyWorkspace(name),
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Workspace name"));
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
        // arrangement only reaches the settings file on the next layout dump,
        // so without this the bundle would capture whatever's stale on disk.
        self.persist(window, cx);
        if crate::workspaces::path_for(&name).exists() {
            self.layout_dialog = Some(LayoutDialog::ConfirmOverwriteWorkspace(name));
            self._layout_input = None;
            cx.notify();
            return;
        }
        crate::workspaces::store(&WorkspaceBundle::from_settings(name, &Settings::load()));
        self.close_layout_dialog(window, cx);
    }

    /// Replace the pending workspace with the current look, the confirm
    /// dialog's yes. Only user bundles reach this confirm, and one deleted
    /// since the dialog opened just comes back: the write lands on the file
    /// the name picks either way.
    fn overwrite_workspace_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmOverwriteWorkspace(name)) => name.clone(),
            _ => return,
        };
        // Flush the live dock so the overwrite captures current panel config,
        // not the stale disk copy. See commit_save_workspace.
        self.persist(window, cx);
        // The bundle's name picks its file, so an overwrite lands back on the
        // same one a first save wrote: both are the one write.
        crate::workspaces::store(&WorkspaceBundle::from_settings(name, &Settings::load()));
        self.close_layout_dialog(window, cx);
    }

    /// Apply the pending workspace to this window, the confirm dialog's yes.
    fn apply_workspace_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &self.layout_dialog {
            Some(LayoutDialog::ConfirmApplyWorkspace(name)) => name.clone(),
            _ => return,
        };
        self.apply_workspace(&name, window, cx);
        self.close_layout_dialog(window, cx);
    }

    /// Pick a workspace file and add it to the collection, the settings
    /// window's Import path from the menu.
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
            this.update(cx, |_, cx| {
                cx.notify();
                native_menu::rebuild(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Toggle the mini layout: on the mini preset the click goes back to the
    /// primary, on anything else it goes to the mini preset. The named preset
    /// is the whole story now, so there is no stash to restore and no separate
    /// flag to flip; whichever side we land on becomes the active layout, and
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
        // On macOS the window may sit in its own fullscreen Space, where
        // AppKit owns the frame: the layout's resize is dropped and the mini
        // player would come up stretched over the whole screen. Leave
        // fullscreen first and apply the layout once the exit transition has
        // landed, so the resize hits a normal window.
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Layout name"));
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
    /// an update to one that already carries the name. An empty name waits.
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
    /// route when the panel is locked. The pin is there to survive stray
    /// clicks, so the entry asks rather than swallowing the click - a menu
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
    /// this one click is what gets through it.
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
        // Every save, overwrite, and apply lands here on its way out, so this
        // is the one place that catches a workspace or layout the native
        // menu's baked-in submenus would otherwise miss. A cancel rebuilds
        // an identical bar, which costs nothing worth branching for.
        native_menu::rebuild(cx);
    }

    /// Open the quick-play modal, or close it when it is already up. The
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

    /// Open the queue modal, or close it when it is already up. The queue
    /// widget calls this when no queue panel is docked, so a click always
    /// lands somewhere. A fresh queue panel each open, dropped on close; its
    /// view (columns, headings) rides settings, so it comes back the way it
    /// was left.
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
    /// Escape both land here.
    fn close_queue_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.queue_modal = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// Keep the window title on the playing track: "artist - title - rox"
    /// while something plays, the plain app name otherwise. Untagged files
    /// fall back to their file name, same as the track info readout.
    fn refresh_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if path == self.titled_track {
            return;
        }
        let title = match &path {
            Some(path) => {
                let meta = self.state.library.read(cx).meta_for(path);
                let track = meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string())
                });
                match meta.map(|m| m.artist).filter(|a| !a.is_empty()) {
                    Some(artist) => format!("{artist} - {track} - rox"),
                    None => format!("{track} - rox"),
                }
            }
            None => "rox".into(),
        };
        window.set_window_title(&title);
        self.titled_track = path;
    }

    /// Route OS-handed files into the shared player. The launch path
    /// (`rox song.flac` and the .desktop actions) lands here after the
    /// window's player exists; paths are already filtered to decodable audio.
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
        let paths = rox_library::open_files::resolve_audio_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.play_now(paths, cx));
    }

    /// Add dropped files or tracks to the up-next queue, filtered to decodable
    /// audio. The Add to queue drop zone routes here.
    fn queue_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let paths = rox_library::open_files::resolve_audio_paths(paths);
        if paths.is_empty() {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| player.enqueue(paths, cx));
    }

    /// The Play now / Add to queue drop zones, shown only while an audio
    /// payload is dragged: a file from the OS (ExternalPaths) or a track from
    /// the library (PlayDrag). Other drags (panel docking, queue reorder)
    /// leave them hidden. Rendered as the top layer so the drop always lands
    /// here - an occluded window-root target misses it because the panels
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
                .child(self.drop_zone("Play now", icons::PLAY, true, cx))
                .child(self.drop_zone("Add to queue", icons::LIST_MUSIC, false, cx))
                .into_any_element(),
        )
    }

    /// One drop zone card. `play_now` true plays the drop after the current
    /// track and jumps to it; false appends it to the queue. Both accept a
    /// file from the OS and a track dragged from the library.
    fn drop_zone(
        &self,
        label: &'static str,
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
            .child(div().text_lg().child(label))
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
                this.play_dropped(drag.paths.to_vec(), cx);
            }))
        } else {
            card.on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.queue_dropped(paths.paths().to_vec(), cx);
            }))
            .on_drop(cx.listener(|this, drag: &PlayDrag, _, cx| {
                this.queue_dropped(drag.paths.to_vec(), cx);
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
        // The playing track rides along as its library id, for the launch
        // restore. Nothing playing, or a file outside the library, clears
        // it: the next launch starts cold.
        let library = self.state.library.read(cx);
        let last_track = self.state.player.read(cx).now_playing().and_then(|now| {
            let id = library.id_for(&now.path)?;
            Some(LastTrack {
                id,
                position_secs: now.position_secs,
            })
        });
        // The whole queue rides along too, as library ids so it survives path
        // changes, keeping each entry's explicit flag and the audible cursor.
        // A file outside the library drops from the order; the cursor tracks
        // the last kept entry at or before it so it stays on the playing
        // track. Everything gone (or nothing playing) clears it and the
        // single-track fallback above carries the restore.
        let last_queue = self.state.player.read(cx).queue_state().and_then(
            |(entries, cursor, position_secs)| {
                let mut tracks = Vec::with_capacity(entries.len());
                let mut new_cursor = 0;
                for (i, (path, explicit)) in entries.iter().enumerate() {
                    if let Some(id) = library.id_for(path) {
                        if i <= cursor {
                            new_cursor = tracks.len();
                        }
                        tracks.push(QueuedTrack {
                            id,
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
    /// tab group at the end - a new group rather than a new tab, so they
    /// sit next to the transport pieces instead of hiding one. The library
    /// stays a center panel: it wants the tall area, and keeping additions
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
        // while it is still attached: stacks skip panels they already hold.
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
    /// wants to be a strip, not a tall tile, so it goes in sized to just its
    /// input row rather than splitting a center panel's height.
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

    /// Whether the dock shows no panels at all - every stack walked down
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
    /// per group. The whole-look pickers live in the welcome window's
    /// quick-start tiles now; this stays the piece-by-piece way in.
    fn empty_hint(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            // An empty dock paints no surface of its own, so without this
            // the launcher's copy would sit straight on the backdrop art.
            // The same page surface the settings pages hold: opaque at
            // full surface opacity, thinning with the rest.
            .bg(palette::bg_elevated())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            // A way back to the welcome window's quick-start tiles, since
            // this launcher replaced it as the empty window's face.
            .child(
                div().absolute().top(tokens::SPACE_SM).right(tokens::SPACE_SM).child(
                    panel::icon_control(
                        icons::INFO,
                        palette::text_muted(),
                        "Open the welcome window",
                        |this: &mut Self, cx| {
                            crate::startup::welcome_window::open(this.state.clone(), cx);
                        },
                        cx,
                    ),
                ),
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
                                        div().text_color(palette::text()).child("An empty window"),
                                    )
                                    .child(
                                        div().text_xs().text_color(palette::text_muted()).child(
                                            "Add your first panel to start building; or chose a preset \
                                            under Workspace > Apply Workspace",
                                        ),
                                    )
                                    // The fallback notice: this window is
                                    // empty because a layout would not
                                    // restore, not because the user asked.
                                    // Accent rather than a status red: the
                                    // app recovered, this is emphasis.
                                    .when_some(self.layout_error, |d, message| {
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
                                def.label,
                                def.icon,
                                catalog::supports_signals(def),
                                cx.listener(move |this, _, window, cx| {
                                    this.run(MenuAction::OpenPanel(def), window, cx);
                                }),
                            )
                        });
                        launcher_section(
                            section.group.map(|(label, _)| label).unwrap_or("Panels"),
                            tiles,
                        )
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
                                    crate::startup::welcome_window::open(
                                        this.state.clone(),
                                        cx,
                                    );
                                }),
                            )
                            .child("Need help?"),
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
            (icons::MAXIMIZE, "Back to the full layout")
        } else {
            (icons::MINIMIZE, "Shrink to the mini player")
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
    /// occluding layer. The save card carries the `SearchInput` key context
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
                .child(div().child("Save Layout"))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Save",
                            true,
                            cx.listener(|this, _, window, cx| this.commit_save(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmOverwrite(name) => card
                .child(div().child(SharedString::from(format!("Overwrite \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the saved layout with the current one."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Overwrite",
                            true,
                            cx.listener(|this, _, window, cx| this.overwrite_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::ConfirmApply(name) => card
                .child(div().child(SharedString::from(format!("Apply \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces this window's current layout."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Apply",
                            true,
                            cx.listener(|this, _, window, cx| this.apply_confirmed(window, cx)),
                        )),
                ),
            LayoutDialog::SaveWorkspace(input) => card
                .key_context("SearchInput")
                .child(div().child("Save Workspace"))
                .child(Input::new(input))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Save",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.commit_save_workspace(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmOverwriteWorkspace(name) => card
                .child(div().child(SharedString::from(format!("Overwrite \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the saved workspace with the current look."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Overwrite",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.overwrite_workspace_confirmed(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmApplyWorkspace(name) => card
                .child(div().child(SharedString::from(format!("Apply \"{name}\"?"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("This replaces the whole look - layouts, palette, appearance."),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Apply",
                            true,
                            cx.listener(|this, _, window, cx| {
                                this.apply_workspace_confirmed(window, cx)
                            }),
                        )),
                ),
            LayoutDialog::ConfirmCloseLocked { name, .. } => card
                .child(div().child(SharedString::from(format!("Close \"{name}\"?"))))
                .child(
                    div().text_xs().text_color(palette::text_muted()).child(
                        "This panel is pinned in place. Closing it takes it out of the layout.",
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(dialog_button(
                            "Cancel",
                            false,
                            cx.listener(|this, _, window, cx| this.close_layout_dialog(window, cx)),
                        ))
                        .child(dialog_button(
                            "Close",
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

/// A muted heading over a divider, grouping the rows below it in a dropdown.
pub(crate) fn menu_section(label: &'static str) -> Div {
    div()
        .mt(tokens::SPACE_XS)
        .pt(tokens::SPACE_XS)
        .px(tokens::SPACE_MD)
        .border_t_1()
        .border_color(palette::border())
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
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
    label: &'static str,
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
        .child(label)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The screen shader's hot reload and signal feed ride the render
        // loop; it keeps its own frame requests going while audio moves.
        self.drive_post_shader(window, cx);
        // A hidden menubar comes back while alt is held, and stays while a
        // dropdown is open so releasing alt can't strand one barless.
        let menubar_hidden = settings::hide_menubar();
        // Alt reveals the hidden bar, but Alt+drag is the compositor's
        // window move/resize; suppress the reveal while a button is down so
        // the overlay never sits in front of the drag. An open menu keeps
        // it up regardless (that press landed on a menu, not a drag).
        let menubar_revealed =
            self.open_menu.is_some() || (window.modifiers().alt && !self.pointer_down);
        // Every panel in this window renders under its player's art tint,
        // and the window claims the one widget theme while it holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        let dock_empty = self.dock_is_empty(cx);
        panel::window_body(player, || {
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
                // ladders (search boxes, quick-play) live in listeners that
                // stop propagation - a binding here would steal their escape.
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
                    if this.dock.update(cx, |dock, cx| dock.zoom_out(window, cx)) {
                        cx.stop_propagation();
                    }
                }))
                // Quit bypasses the window close hook, so dump the layout and
                // frame here or a pending debounce and any window move since
                // the last save are lost.
                .on_action(cx.listener(|this, _: &Quit, window, cx| {
                    this.persist(window, cx);
                    cx.quit();
                }))
                // Alt reveals a hidden menubar, so modifier flips repaint. Gated
                // on the setting so the common case stays free of repaints.
                .on_modifiers_changed(cx.listener(|_, _, _, cx| {
                    if settings::hide_menubar() {
                        cx.notify();
                    }
                }))
                // Track the mouse button in the capture phase so the
                // alt-revealed bar can duck a window move/resize drag. Capture
                // beats the occluding overlay and any panel that eats the
                // press; only the alt-reveal path cares, so repaint just there.
                .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
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
                .when(!menubar_hidden, |d| d.child(self.menubar(cx)))
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
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .occlude()
                            .child(self.menubar(cx)),
                    )
                })
                // The quick-play modal floats over everything on an occluding
                // layer, so a click outside it dismisses without also landing
                // on whatever sits underneath. Not deferred: it is the last
                // child so it already paints on top, and the search box's
                // suggestion popover defers itself - gpui panics on a
                // defer_draw inside a deferred draw. `overlay_phase` is what
                // buys the rest of that deal: painting last puts it over the
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
                // sit on top of every panel, which also makes them the topmost
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

    /// A hub carrying one band signal, ticked once so the engine has a slot
    /// to read. Silent: what's being checked here is which path fills the
    /// slots, and a route's Quiet end is what it reads at silence, which
    /// makes the two paths tell themselves apart with no audio at all.
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

    /// Both paths in one test on purpose: they share a process-wide static,
    /// and two tests setting it would race each other in the same binary.
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

        // A route pointing at a signal the pool never carried leaves its
        // slot alone rather than falling back to the pool order.
        set_post_shader_routes(vec![Route {
            enabled: true,
            signal: id + 99,
            target: panel::shader::slot_target(1),
            from: 0.5,
            to: 1.0,
        }]);
        assert_eq!(post_shader_signals(&hub), [0.0; panel::shader::SLOTS]);
        set_post_shader_routes(Vec::new());
    }
}
