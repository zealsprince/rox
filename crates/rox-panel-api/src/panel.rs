//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use gpui::{
    anchored, deferred, div, linear_color_stop, linear_gradient, prelude::*, px, relative, size,
    AbsoluteLength, AnyElement, App, Bounds, Context, DismissEvent, Div, Element, Entity,
    FocusHandle, Focusable as _, GlobalElementId, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Rgba, SharedString, Size, Stateful,
    Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Root};
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::actions::{SeekBackward, SeekForward, TogglePlayback};
use crate::query::shared_query::SharedQuery;
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette::PanelTheme;
use rox_design::{palette, tokens};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::discord_presence::DiscordPresence;
use rox_services::history::History;
use rox_services::lastfm::Scrobbler;
use rox_services::player::{fmt_time, FadeView, Player};
use rox_services::portraits::Portraits;
use rox_services::selection::Selection;
use rox_services::thumbs::Thumbs;

pub mod arrange;
pub use arrange::*;

pub mod shader;
pub use shader::PanelShader;

// The widget layer lives in rox-panel-kit now. Panels reach it through
// crate::panel the way they always have, so the split stays behind this
// line.
pub use rox_panel_kit::{
    align_row, banner, banner_flow, check_row, choices, choices_gated, display_name,
    flick_on_paint_axis, follow_panel, font_picker, glide_snap_axis, glide_step, glide_step_axis,
    glide_target, glide_target_axis, icon_choices, icon_control, icon_control_sized, icon_toggles,
    items, justify, justify_v, mode_list, paint_slider, picker, scrub_on_paint, setting_block,
    setting_row, setting_row_dyn, title_text, toggle, toggle_face, toggle_locked, tracking_section,
    type_ahead_grow, valign_row, value_slider_edit, value_slider_edit_over,
    value_slider_edit_sized, window_body, Align, FlickState, ModeSpec, ResumeIdle, ScrubState,
    SliderWidth, Tip, Tone, TrackedImage, VAlign, ValueEdit,
};

/// The shared entities every panel renders over: one player, one catalog,
/// and one selection per workspace. Cloning shares the handles, not the
/// state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
    pub selection: Entity<Selection>,
    /// The app-wide search query the global-following panels share.
    pub query: Entity<SharedQuery>,
    pub tab_hosts: Entity<TabHosts>,
    /// The playing track's art baked into the window backdrop, one bake
    /// shared by every window over this player.
    pub now_art: Entity<NowPlayingArt>,
    /// The artwork service's texture cache, shared by every view that
    /// draws cover thumbnails.
    pub thumbs: Entity<Thumbs>,
    /// The artist portrait cache, shared by every view that draws faces:
    /// the artist wall and the stats page.
    pub portraits: Entity<Portraits>,
    /// The Last.fm scrobbler over this workspace's player; also where the
    /// live scrobble config lives, for the panels' threshold markers.
    pub scrobbler: Entity<Scrobbler>,
    /// The listen recorder riding the scrobbler's listen signal; history
    /// views subscribe to it for the refresh when an event lands.
    pub history: Entity<History>,
    /// Discord Rich Presence publisher watching the player.
    pub discord: Entity<DiscordPresence>,
    /// The shared signal pool and its engine: the app-wide modulation
    /// sources any panel's parameters can ride. Panels tick it from their
    /// paint and read values; edits persist through settings.
    pub signals: Arc<rox_viz::signal::SignalHub>,
}

/// Every tab panel that has hosted one of our panels, reported from each
/// panel's `on_added_to`. Dragging a tab into a split makes the dock create
/// tab panels on its own and nothing announces them to the workspace, so
/// this registry is how it finds them, to pick a live tab panel for
/// Panels-menu additions.
#[derive(Default)]
pub struct TabHosts {
    hosts: Vec<WeakEntity<TabPanel>>,
}

impl TabHosts {
    /// Record a hosting tab panel.
    pub fn report(&mut self, tabs: WeakEntity<TabPanel>) {
        if self.hosts.iter().any(|t| t.entity_id() == tabs.entity_id()) {
            return;
        }
        self.hosts.push(tabs);
    }

    /// The newest recorded tab panel that is still alive and showing panels.
    pub fn last_live(&self, cx: &App) -> Option<Entity<TabPanel>> {
        self.hosts.iter().rev().find_map(|tabs| {
            let tabs = tabs.upgrade()?;
            tabs.read(cx).visible(cx).then_some(tabs)
        })
    }
}

/// Jump to an open panel by its built-in name across every tab group that has
/// hosted our panels: make the first live match the active, focused tab, and
/// return whether one was found. The queue widget uses it to reach an open
/// queue panel before falling back to a window. Popped-out panels live in
/// their own windows rather than the dock, so they are not matched here.
pub fn focus_panel_named(
    hosts: &Entity<TabHosts>,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let groups = hosts.read(cx).hosts.clone();
    for tabs in groups {
        let Some(tabs) = tabs.upgrade() else { continue };
        let target = tabs
            .read(cx)
            .panels()
            .iter()
            .find(|panel| panel.panel_name(cx) == name && panel.visible(cx))
            .cloned();
        if let Some(panel) = target {
            tabs.update(cx, |tabs, cx| tabs.focus_panel(&panel, window, cx));
            return true;
        }
    }
    false
}

/// [`icon_control`] that shows a crossfade running through it: while two
/// tracks overlap, an accent wash sweeps across the button in the direction
/// the skip went, its soft edge sitting where the fade has got to. The
/// control that started the overlap is the one that shows it, so the
/// animation says which way the queue moved as well as how much is left.
/// None is the plain button.
pub fn icon_control_fading<V: 'static>(
    icon: &'static str,
    color: Rgba,
    tip: impl Into<Tip>,
    fade: Option<FadeView>,
    outro: Option<f32>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    icon_control_sized(icon, px(16.), color, tip, on_click, cx)
        .when_some(fade, |d, fade| {
            // Soft-edged rather than a hard wipe: the thing being drawn is a
            // fade, and an edge that blurs across the button reads as one where
            // a moving hard line reads as a progress bar.
            let wash = palette::alpha(palette::accent(), 0x66);
            let clear = palette::alpha(palette::accent(), 0x00);
            let at = fade.progress();
            d.bg(linear_gradient(
                // 90 runs left to right, 270 the other way, so a Previous
                // sweeps back the way it sent the queue.
                if fade.back { 270. } else { 90. },
                linear_color_stop(wash, (at - EDGE).max(0.0)),
                linear_color_stop(clear, (at + EDGE).min(1.0)),
            ))
        })
        // The sweep's exit: a completed fade leaves the whole button washed,
        // and cutting that to nothing reads as a glitch. Instead it lands
        // one notch brighter than the sweep it ends (the flash) and
        // dissolves. Flat rather than the gradient, since the sweep already
        // arrived; this is the settle, not more motion.
        .when_some(outro, |d, strength| {
            d.bg(palette::alpha(
                palette::accent(),
                (0x99 as f32 * strength) as u8,
            ))
        })
}

/// How far either side of the fade's position the sweep's edge blurs, as a
/// fraction of the button.
const EDGE: f32 = 0.2;

/// Map a strip fraction to an absolute seek on the playing track, the
/// seek strip's and the waveform's shared apply.
pub fn seek_fraction(player: &Entity<Player>, fraction: f32, cx: &App) {
    let player = player.read(cx);
    let Some(now) = player.now_playing() else {
        return;
    };
    let Some(duration) = now.duration_secs else {
        return;
    };
    player.seek_to(fraction as f64 * duration);
}

/// A seek preview for a scrub strip: the time under the pointer as a small
/// pill that follows the cursor while hovering. Tracks the pointer across
/// `scrub`'s painted bounds and maps it against `duration`. Drop it as a
/// child over the strip's relative container - it covers the strip to catch
/// every move, and a click through it bubbles to the strip's own seek
/// handler underneath.
pub fn seek_hover<V: 'static>(
    scrub: &ScrubState,
    duration: f64,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let moved = scrub.clone();
    let left = scrub.clone();
    let hover = scrub.hover();
    div()
        // The id makes the element stateful, which the hover-leave catch
        // below needs.
        .id("seek-hover")
        .absolute()
        .inset_0()
        .cursor_pointer()
        .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            if moved.set_hover(moved.fraction(event.position.x)) {
                cx.notify();
            }
        }))
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            // The pointer left the strip: no more move events fire, so the
            // leave has to clear the readout itself.
            if !hovered && left.set_hover(None) {
                cx.notify();
            }
        }))
        .when_some(hover, |d, fraction| d.child(seek_pill(fraction, duration)))
}

/// The seek preview label: the time at `fraction` along the track, a pill
/// centered over that point near the top of the strip. A zero-width column
/// at the fraction centers the pill on the cursor line.
fn seek_pill(fraction: f32, duration: f64) -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(relative(fraction))
        .w_0()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                // The zero-width column above gives the text no room, so a
                // multi-digit time would wrap to one glyph per line without
                // this.
                .whitespace_nowrap()
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded(tokens::RADIUS)
                .bg(palette::bg_menu_opaque())
                .border_1()
                .border_color(palette::border())
                .text_sm()
                .text_color(palette::text())
                .child(fmt_time(fraction as f64 * duration)),
        )
}

/// Repaint the tab panel hosting a renamed panel. The tab bar draws the
/// title, and that row only repaints when the tab panel itself is
/// notified; the panel's own notify never reaches it.
pub fn refresh_tab_panel(tab_panel: &Option<WeakEntity<TabPanel>>, cx: &mut App) {
    if let Some(tabs) = tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |_, cx| cx.notify());
    }
}

/// Read a panel's config back out of a dumped panel state; anything
/// missing or malformed falls back to defaults.
pub fn config_from_info<C: Default + serde::de::DeserializeOwned>(info: &PanelInfo) -> C {
    match info {
        PanelInfo::Panel(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        _ => C::default(),
    }
}

/// The Pop Out and Close tail of a panel's dropdown menu: out of the dock
/// into an OS window, or out of the layout entirely. Pass the tab panel
/// the panel currently sits in (from `on_added_to`); the state is what
/// Dock Back later reaches the workspace through.
///
/// Close lives on this tail rather than the dock's menus so every panel
/// carries it everywhere its menu shows - for a solo content panel (no
/// tab chrome, and its content's own context menu replaces the dock's
/// body menu) this is the only close there is, and the empty window it
/// can leave behind offers the way back in. Popped out there is no Close:
/// closing the OS window is the close. On a pinned panel the click puts up a
/// confirm and closes from there, so the pin costs a second click rather than
/// eating the first.
/// The Dock Back entry: the popped-out counterpart of Pop Out. Moves the
/// panel into the workspace's newest live tab group and closes the window it
/// was hosted in (harmless if there is none). Cross-window drags can't carry
/// a panel home - a held button pins pointer events to its window and Wayland
/// hides window positions - so this menu is the way back. It no-ops when the
/// layout has no live tab group to land in.
pub fn dock_back_item(menu: PopupMenu, panel: Arc<dyn PanelView>, state: AppState) -> PopupMenu {
    let hosts = state.tab_hosts.clone();
    menu.item(
        PopupMenuItem::new("Dock Back")
            .icon(Icon::default().path(icons::EXTERNAL_LINK))
            .on_click(move |_, window, cx| {
                let Some(tabs) = hosts.read(cx).last_live(cx) else {
                    return;
                };
                tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel.clone(), window, cx);
                });
                window.remove_window();
            }),
    )
}

pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &Window,
) -> PopupMenu {
    // No tab strip means the panel is either in a window of its own or
    // hosted in a composite's slot. In a window it popped out into, the item
    // that belongs here is the way home rather than another Pop Out;
    // anywhere else there is no home to name, so the tail ends here.
    let Some(tabs) = tab_panel.clone() else {
        if !dock_back_offered(window) {
            return menu;
        }
        // Kept out of design mode, unlike the two rows below: a panel in a
        // window of its own has no other menu and no menubar behind it, so
        // dropping this would leave it with no way back into the layout at
        // all. It's the exit from a stranded window rather than a way to
        // rearrange a finished one.
        return dock_back_item(menu, Arc::new(panel.clone()), state);
    };
    if !settings::design_mode() {
        return menu;
    }
    let pop_panel = panel.clone();
    let pop_tabs = tab_panel;
    let menu = menu.item(
        PopupMenuItem::new("Pop Out")
            .icon(Icon::default().path(icons::EXTERNAL_LINK))
            .on_click(move |_, window, cx| {
                pop_out(
                    pop_panel.clone(),
                    pop_tabs.clone(),
                    state.clone(),
                    window,
                    cx,
                );
            }),
    );
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Close")
            .icon(Icon::default().path(icons::CLOSE))
            .on_click(move |_, window, cx| {
                if panel.read(cx).locked(cx) {
                    // The pin exists to survive a stray click, so route the
                    // click to a confirm rather than dropping it. Without a
                    // workspace behind the window there is nowhere to float
                    // the dialog, and the pin holds as it did before.
                    let view: Arc<dyn PanelView> = Arc::new(panel.clone());
                    crate::openers::confirm_close_locked(view, tabs.clone(), window, cx);
                    return;
                }
                let Some(tabs) = tabs.upgrade() else {
                    return;
                };
                tabs.update(cx, |tabs, cx| {
                    tabs.remove_panel(Arc::new(panel.clone()), window, cx);
                });
            }),
    )
}

/// The Duplicate entry for a panel's dropdown menu: drops a second panel of
/// the same type into this one's tab strip, carrying the config along so the
/// copy opens configured the same. Each panel's `new` takes a different
/// shape, so `make` reconstructs the copy from the source panel - typically
/// cloning its state and config, then calling the panel's own constructor.
///
/// A panel with no tab strip - popped out into a window, or hosted in a
/// composite's slot - has nowhere to put the copy, so it gets no entry at
/// all. The row used to draw there and do nothing on click.
pub fn duplicate_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    make: impl Fn(&Entity<P>, &mut Window, &mut Context<P>) -> P + 'static,
) -> PopupMenu {
    if tab_panel.is_none() || !settings::design_mode() {
        return menu;
    }
    let weak = panel.downgrade();
    menu.item(
        PopupMenuItem::new("Duplicate")
            .icon(Icon::default().path(icons::COPY))
            .on_click(move |_, window, cx| {
                let Some(this) = weak.upgrade() else { return };
                let Some(tabs) = tab_panel.clone().and_then(|tabs| tabs.upgrade()) else {
                    return;
                };
                let dup = cx.new(|cx| make(&this, window, cx));
                tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
            }),
    )
}

/// The Reveal in File Browser entry for a track context menu: shows the
/// track's file in the platform file manager, which lands in its album
/// folder. The id resolves to its path at click time, so the reveal
/// follows a file the library has since re-scanned elsewhere; None (an
/// empty selection) appends nothing.
pub fn reveal_item(menu: PopupMenu, state: AppState, id: Option<i64>) -> PopupMenu {
    let Some(id) = id else {
        return menu;
    };
    menu.item(
        PopupMenuItem::new("Reveal in File Browser")
            .icon(Icon::default().path(icons::FOLDER))
            .on_click(move |_, _, cx| {
                let path = state
                    .library
                    .read(cx)
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop());
                if let Some(path) = path {
                    cx.reveal_path(&path);
                }
            }),
    )
}

/// Resolve track ids to keys and hand them to the player: after the playing
/// track when `next`, at the tail otherwise. Shared by the context-menu
/// actions across every song surface.
pub fn queue_tracks(state: &AppState, ids: &[i64], next: bool, cx: &mut App) {
    let keys = match state.library.read(cx).keys_for(ids) {
        Ok(keys) if !keys.is_empty() => keys,
        _ => return,
    };
    state.player.update(cx, |player, cx| {
        if next {
            player.play_next(keys, cx);
        } else {
            player.enqueue(keys, cx);
        }
    });
}

/// The track actions every song surface's right-click shares: Play under
/// the caller's label, the selection into the tag and cover editors, and
/// Reveal in File Browser. What playing queues differs per panel (the
/// view from a row, the highlighted set, whole albums), so the caller
/// hands the click over; everything after acts on the ids, resolved at
/// build time so the editors get this set even if another panel
/// publishes over the shared selection before the click lands. Reveal
/// follows the first id; empty ids appends no Reveal.
pub fn track_actions(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    play_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    on_play: impl Fn(&mut Window, &mut App) + 'static,
) -> PopupMenu {
    let reveal = ids.first().copied();
    let tag_ids = ids.clone();
    let tag_state = state.clone();
    let cover_state = state.clone();
    let rename_ids = ids.clone();
    let rename_state = state.clone();
    let convert_ids = ids.clone();
    let convert_state = state.clone();
    let next_state = state.clone();
    let next_ids = ids.clone();
    let queue_state = state.clone();
    let queue_ids = ids.clone();
    let playlist_state = state.clone();
    let playlist_ids = ids.clone();
    let menu = menu
        .item(
            PopupMenuItem::new(play_label)
                .icon(Icon::default().path(icons::PLAY))
                .on_click(move |_, window, cx| on_play(window, cx)),
        )
        // Queue the selection right after the playing track, or start it when
        // nothing plays. Paths resolve here so the queue holds the same set
        // even if the selection moves before the click lands.
        .item(
            PopupMenuItem::new("Play Next")
                .icon(Icon::default().path(icons::SKIP_FORWARD))
                .on_click(move |_, _, cx| {
                    queue_tracks(&next_state, &next_ids, true, cx);
                }),
        )
        .item(
            PopupMenuItem::new("Add to Queue")
                .icon(Icon::default().path(icons::LIST_MUSIC))
                .on_click(move |_, _, cx| {
                    queue_tracks(&queue_state, &queue_ids, false, cx);
                }),
        );
    // The favourites toggle: off to on when any of the set is not favourited,
    // on to off only when the whole set already is, so a mixed selection lands
    // everything in favourites first. Reads its state at open time.
    let favourites = state.library.read(cx).favourite_ids();
    let all_fav = !ids.is_empty() && ids.iter().all(|id| favourites.contains(id));
    let fav_state = state.clone();
    let fav_ids = ids.clone();
    let (fav_label, fav_icon) = if all_fav {
        ("Remove from Favourites", icons::HEART_FILLED)
    } else {
        ("Add to Favourites", icons::HEART)
    };
    let menu = menu.item(
        PopupMenuItem::new(fav_label)
            .icon(Icon::default().path(fav_icon))
            .on_click(move |_, _, cx| {
                let ids = fav_ids.clone();
                fav_state
                    .library
                    .update(cx, |library, cx| library.set_favourites(&ids, !all_fav, cx));
            }),
    );
    // Add to Playlist flies out the existing playlists with Create New at the
    // top. Built at open time, so it reflects playlists made this session.
    let submenu = PopupMenu::build(window, cx, move |mut submenu, _window, cx| {
        let new_state = playlist_state.clone();
        let new_ids = playlist_ids.clone();
        submenu = submenu.item(
            PopupMenuItem::new("New Playlist...")
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    crate::openers::playlist_create(new_state.clone(), new_ids.clone(), cx);
                }),
        );
        // Static lists only: a smart playlist holds what its query answers,
        // so there is nothing here for a track to be added to.
        let playlists: Vec<_> = playlist_state
            .library
            .read(cx)
            .playlists()
            .into_iter()
            .filter(|playlist| playlist.kind == rox_library::playlists::PlaylistKind::Static)
            .collect();
        if !playlists.is_empty() {
            submenu = submenu.separator();
        }
        for playlist in playlists {
            let add_state = playlist_state.clone();
            let add_ids = playlist_ids.clone();
            let id = playlist.id;
            submenu = submenu.item(
                PopupMenuItem::new(SharedString::from(playlist.name)).on_click(move |_, _, cx| {
                    let add_ids = add_ids.clone();
                    add_state.library.update(cx, |library, cx| {
                        library.add_to_playlist(id, &add_ids, cx);
                    });
                }),
            );
        }
        submenu
    });
    let menu = menu.item(
        PopupMenuItem::submenu("Add to Playlist", submenu)
            .icon(Icon::default().path(icons::LIST_MUSIC)),
    );
    let menu = menu
        // The primary editing flow: the selection into the tag editor
        // window; the metadata panel's inline pencil stays the quick path.
        .item(
            PopupMenuItem::new("Edit Tags...")
                .icon(Icon::default().path(icons::PENCIL))
                .on_click(move |_, _, cx| {
                    crate::openers::tags_editor(tag_state.clone(), tag_ids.clone(), cx);
                }),
        )
        // Covers get their own window: the tag editor edits text per
        // track, this stamps one image across the selection.
        .item(
            PopupMenuItem::new("Edit Cover Art...")
                .icon(Icon::default().path(icons::IMAGE))
                .on_click(move |_, _, cx| {
                    crate::openers::cover_editor(cover_state.clone(), ids.clone(), cx);
                }),
        )
        // The other direction: tags into filenames. Renaming is a disk
        // change rather than a tag edit, so it gets its own dialog with
        // the whole plan on screen before anything moves.
        .item(
            PopupMenuItem::new("Rename Files...")
                .icon(Icon::default().path(icons::FOLDER))
                .on_click(move |_, _, cx| {
                    crate::openers::rename_dialog(rename_state.clone(), rename_ids.clone(), cx);
                }),
        );
    // Converting writes new files somewhere else entirely, so it only shows
    // up where the encoder to write them exists. No ffmpeg, no row.
    let menu = if crate::openers::convert_available() {
        menu.item(
            PopupMenuItem::new("Convert...")
                .icon(Icon::default().path(icons::AUDIO_LINES))
                .on_click(move |_, _, cx| {
                    crate::openers::convert_dialog(convert_state.clone(), convert_ids.clone(), cx);
                }),
        )
    } else {
        menu
    };
    reveal_item(menu, state, reveal)
}

/// Move a docked panel into its own OS window. The panel entity itself moves,
/// so it keeps rendering the same shared state; closing the window drops it.
pub fn pop_out<P: Panel>(
    panel: Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &mut Window,
    cx: &mut App,
) {
    // Detach from the dock first; the new window's host keeps the entity
    // alive from here on.
    if let Some(tabs) = tab_panel.and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(Arc::new(panel.clone()), window, cx);
        });
    }
    pop_out_view(Arc::new(panel), state, cx);
}

/// Open an OS window hosting an already-detached panel. Also the dock's
/// middle-drag-out hook: dragging a panel out of the window lands here.
/// The window title comes from the panel's rename when one is set, its
/// built-in name otherwise.
pub fn pop_out_view(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    panel_window(panel, state, false, cx);
}

/// Open a panel straight into a window of its own, never having been in a
/// layout: the Window menu's New Window from Panel. The same window as a
/// pop-out apart from the way back, which this one has no use for - the
/// panel didn't come out of a dock, so there is nowhere to send it back to.
pub fn open_panel_window(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    panel_window(panel, state, true, cx);
}

/// The windows hosting a single panel, and whether that panel came out of a
/// dock: true for a pop-out, false for one opened straight into a window.
/// Only the first has somewhere to go back to. Read from menu builders that
/// have a window and no `App`, which is why it's a static rather than a gpui
/// global; entries go when the window's host drops.
static PANEL_WINDOWS: RwLock<BTreeMap<u64, bool>> = RwLock::new(BTreeMap::new());

/// Whether a menu built in `window` should offer the way back into a dock:
/// only in a panel window, and only one the panel was popped out into. A
/// panel with no tab strip that isn't in one of these is a composite's
/// hosted child, sitting in a window full of other panels - Dock Back there
/// would close the window out from under all of them.
fn dock_back_offered(window: &Window) -> bool {
    PANEL_WINDOWS
        .read()
        .unwrap()
        .get(&window.window_handle().window_id().as_u64())
        .copied()
        .unwrap_or(false)
}

fn panel_window(panel: Arc<dyn PanelView>, state: AppState, fresh: bool, cx: &mut App) {
    let name = panel
        .tab_name(cx)
        .unwrap_or_else(|| display_name(panel.panel_name(cx)).into());
    let title = SharedString::from(format!("rox - {name}"));
    let bounds = Bounds::centered(None, size(px(900.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.clone()),
            ..Default::default()
        }),
        app_id: Some(rox_core::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title(&title);
        // A popped-out panel keeps its surface shader, so this window needs
        // the hub and player its slots read from.
        shader::note_window(window, &state, cx);
        let window_id = window.window_handle().window_id().as_u64();
        PANEL_WINDOWS.write().unwrap().insert(window_id, !fresh);
        let host = cx.new(|cx| {
            // A popped-out window pumps its own frames, so the backdrop
            // needs its own wake on a new bake.
            let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
            PopoutHost {
                panel_view: panel,
                state,
                backdrop: WindowBackdrop::default(),
                context_menu: None,
                focus: cx.focus_handle(),
                window_id,
                _backdrop_changed,
            }
        });
        // Anchor the window on the fallback focus so the Workspace-scoped
        // playback bindings have a dispatch path before the panel grabs
        // focus, same as the main workspace's fallback.
        host.read(cx).focus.clone().focus(window);
        cx.new(|cx| Root::new(host, window, cx))
    })
    .expect("failed to open the panel window");
}

/// Open a child window titled `title`, sized to `bounds`, hosting the view
/// `build` returns wrapped in a Root. Carries the app id so the compositor
/// groups it with the main window, and re-sets the title after creation
/// because the Wayland backend ignores the creation-time titlebar title -
/// the one place that workaround now lives. `min_size` floors an interactive
/// resize; None leaves a fixed-size modal free. The caller keeps its own
/// singleton bookkeeping and stores the returned handle.
pub fn open_child_window<V: 'static + Render>(
    cx: &mut App,
    title: impl Into<SharedString>,
    bounds: Bounds<Pixels>,
    min_size: Option<Size<Pixels>>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
) -> WindowHandle<Root> {
    open_window(cx, title, bounds, min_size, true, build)
}

/// Like [`open_child_window`] but fixed: the user can't resize it, so it
/// holds the bounds it opened at and its min size is that same size. For
/// dialogs whose layout is one set size, like About, where a resize would
/// only strand the content in empty space.
pub fn open_fixed_window<V: 'static + Render>(
    cx: &mut App,
    title: impl Into<SharedString>,
    bounds: Bounds<Pixels>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
) -> WindowHandle<Root> {
    open_window(cx, title, bounds, Some(bounds.size), false, build)
}

fn open_window<V: 'static + Render>(
    cx: &mut App,
    title: impl Into<SharedString>,
    bounds: Bounds<Pixels>,
    min_size: Option<Size<Pixels>>,
    resizable: bool,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
) -> WindowHandle<Root> {
    let title = title.into();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: min_size,
        is_resizable: resizable,
        titlebar: Some(TitlebarOptions {
            title: Some(title.clone()),
            ..Default::default()
        }),
        app_id: Some(rox_core::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        window.set_window_title(&title);
        let view = build(window, cx);
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open child window")
}

/// The frame-level config every panel carries, flattened into each
/// panel's own config struct with `#[serde(flatten)]`. These are the
/// knobs that mean the same thing on any panel: the rename, the palette
/// override, and the two placement locks. Panel-specific fields (a
/// grid's tile size, a spectrum's bands) stay on the panel's own config;
/// `align` lives there too since only some panels lay out along a row.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PanelChrome {
    /// The rename shown as the tab and title text; None shows the
    /// built-in name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The panel's palette and frame override.
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
    /// Pin the panel in place: the dock won't let it be dragged to
    /// another spot or rearranged. Off by default. Resizing is a separate
    /// concern the dock handles at the split level.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Turn the panel body into a window-move handle: a drag anywhere on
    /// it moves the OS window, so a decorations-off layout can be moved by
    /// a toolbar strip. Off by default; meant for the quiet panels, since
    /// on an interactive one it competes with the controls.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anchor: bool,
    /// Drop the in-panel controls a panel floats over its content: a
    /// composition host's corner slot buttons and grip, the metadata
    /// panel's edit toolbar. Off by default, so a panel stays editable
    /// where it sits. On, it reads as finished furniture instead of a
    /// builder's frame, which is what a shipped workspace wants; the
    /// layout is still edited from the Workspace page's tree in Settings.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hide_controls: bool,
    /// Cap the panel's width in px. Set, the dock won't grow the panel wider
    /// than this, and a growing window hands the extra room to its
    /// neighbors instead, so a toolbar pinned narrow stays narrow. None
    /// leaves the width free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    /// Cap the panel's height in px, the vertical twin of
    /// [`max_width`](Self::max_width): what keeps a menu bar or footer from
    /// stretching when the window gets taller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    /// Hold the panel's width to at least this many px, so a resize can't
    /// squeeze it narrower. Raised over the panel's built-in floor, never
    /// below it. None leaves the width at that floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    /// Hold the panel's height to at least this many px, the vertical twin of
    /// [`min_width`](Self::min_width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
    /// A WGSL shader over the panel's own surface, run after its body
    /// paints. None on every panel that has never been given one, which is
    /// what keeps older layout dumps loading clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<PanelShader>,
}

impl PanelChrome {
    /// Whether the panel's in-place editing controls stay off this frame:
    /// its own [`hide_controls`](Self::hide_controls), or design mode being
    /// off, which says the same thing about every panel at once. The one
    /// place that decides, so a panel only ever asks this rather than
    /// reading the field.
    pub fn controls_hidden(&self) -> bool {
        self.hide_controls || !settings::design_mode()
    }
}

/// The panel's size cap as a [`Size`], reading the chrome's optional
/// width/height limits over the panel's own minimum, so a cap can never
/// drop below what the panel needs. An unset axis stays unbounded. Every
/// panel returns this from its `Panel::max_size`, so the cap is a generic
/// panel setting rather than a per-panel opt-in.
///
/// The cap is floored against [`chrome_min_size`] rather than the raw
/// `floor`, because nothing stops a settings file from asking for a min
/// of 500 and a max of 300. The dock can't do anything sane with min >
/// max, so the min wins, the same way the panel's built-in floor beats
/// both.
pub fn chrome_max_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let min = chrome_min_size(chrome, floor);
    let axis = |cap: Option<f32>, min: Pixels| match cap {
        Some(px_value) => px(px_value).max(min),
        None => Pixels::MAX,
    };
    gpui::size(
        axis(chrome.max_width, min.width),
        axis(chrome.max_height, min.height),
    )
}

/// The panel's minimum size as a [`Size`], the chrome's optional min
/// width/height raised over `floor` (the panel's built-in minimum, what its
/// controls need). A user min can only tighten the floor upward, never below
/// it. An unset axis stays at the floor. Every panel returns this from its
/// `Panel::min_size`, the mirror of [`chrome_max_size`].
pub fn chrome_min_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let axis = |min: Option<f32>, floor: Pixels| match min {
        Some(px_value) => px(px_value).max(floor),
        None => floor,
    };
    gpui::size(
        axis(chrome.min_width, floor.width),
        axis(chrome.min_height, floor.height),
    )
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A panel whose per-view config is edited in its own settings window
/// (see the panel settings window): the panel's own pages of control
/// rows, then the shared Appearance page editing the panel's palette
/// override. New knobs go on the panel's config struct and get a row on
/// one of its pages.
pub trait PanelSettings: Panel {
    /// The shared state, so the settings window can back itself with
    /// the playing track's art like every other window.
    fn state(&self) -> AppState;

    /// The panel's own pages as name and sidebar icon pairs, listed
    /// above the shared Appearance page. Empty means the panel has no
    /// knobs beyond its appearance.
    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Whether the settings window offers the shared surface-shader page.
    /// On for every panel by default; a panel whose body already is a
    /// shader opts out rather than wearing two.
    fn surface_shader(&self) -> bool {
        true
    }

    /// One of the panel's own pages: control rows editing the config in
    /// place. Changes apply live; the layout dump persists them.
    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (page, window, cx);
        div().into_any_element()
    }

    /// The panel's frame-level config. Every panel stores a
    /// [`PanelChrome`] on its own config (flattened into the layout dump),
    /// so the shared knobs - rename, theme, the placement locks - read and
    /// write through here rather than a method per field.
    fn chrome(&self) -> &PanelChrome;

    /// The mutable frame config, so the settings window and quick toggles
    /// edit the shared knobs in place.
    fn chrome_mut(&mut self) -> &mut PanelChrome;

    /// The rename override, shown as the tab and title text in place of
    /// the panel's built-in name.
    fn custom_title(&self) -> Option<&str> {
        self.chrome().title.as_deref()
    }

    /// Store an edited rename: the next render shows it, the layout dump
    /// persists it. None goes back to the built-in name. Implementations
    /// must repaint their hosting tab panel ([`refresh_tab_panel`]), which
    /// is what draws the title, so this stays panel-provided.
    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>);

    /// Whether the panel draws its own font control on its pages, so the
    /// shared Appearance page leaves off the generic theme-font row rather
    /// than showing a second family picker. The lyrics panel does, pairing
    /// the family with its own weight and size knobs.
    fn has_own_font(&self) -> bool {
        false
    }

    /// The panel's palette override, the Appearance page's subject.
    fn theme(&self) -> PanelTheme {
        self.chrome().theme.clone()
    }

    /// Store an edited override: the next render picks it up, the layout
    /// dump persists it.
    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.chrome_mut().theme = theme;
        cx.notify();
    }

    /// Pin or unpin the panel in the dock (no drag or rearrange). The dock
    /// reads the flag through [`Panel::locked`] on its next paint, so a
    /// repaint settles the toggle. The current value reads off
    /// `chrome().locked` directly, which also sidesteps the name clash
    /// with the dock trait's own `locked`.
    fn set_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().locked = on;
        cx.notify();
    }

    /// Turn the window-move handle on or off; `chrome().anchor` reads it.
    fn set_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().anchor = on;
        cx.notify();
    }

    /// Whether this panel hosts others and so draws the corner slot
    /// controls. Only the composition hosts override it; it gates the
    /// setting row that hides them, which would be a dead switch anywhere
    /// else.
    fn composite(&self) -> bool {
        false
    }

    /// Show or hide the composition host's corner controls.
    fn set_hide_controls(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().hide_controls = on;
        cx.notify();
    }

    /// Store the panel's width cap in px (None clears it). Repainting the
    /// dock re-reads the cap when it rebuilds the split's size range, so a
    /// repaint settles the change.
    fn set_max_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_width = px;
        cx.notify();
    }

    /// Store the panel's height cap in px (None clears it), the twin of
    /// [`set_max_width`](Self::set_max_width).
    fn set_max_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_height = px;
        cx.notify();
    }

    /// Store the panel's minimum width in px (None clears it), the floor a
    /// resize can't squeeze it below. Same repaint-settles-it path as the
    /// caps.
    fn set_min_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_width = px;
        cx.notify();
    }

    /// Store the panel's minimum height in px (None clears it), the twin of
    /// [`set_min_width`](Self::set_min_width).
    fn set_min_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_height = px;
        cx.notify();
    }

    /// The panel's own rows for the shared Appearance page, rendered as
    /// a section between the frame and the colors: looks that live on
    /// the panel's config rather than its theme, like the grid's art
    /// rounding. None keeps the page to the shared knobs.
    fn appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }

    /// The panel's own rows for the shared Behavior page, rendered under
    /// the shared lock and anchor toggles: knobs about how the panel acts
    /// rather than how it looks, like the grid's follow-playing. None
    /// keeps the page to the shared knobs.
    fn behavior(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }
}

/// How far a press drags before an anchored panel hands the window to the
/// compositor. Under the slop the press stays a click for whatever control
/// sits in the panel, so an anchor over a search box or a button row works
/// like a hidden macOS titlebar: click to use, drag to move.
const ANCHOR_SLOP: Pixels = px(6.);

thread_local! {
    /// The pending anchor drag: which window took the press and where it
    /// landed. One pointer means at most one pending drag, and events
    /// dispatch on the UI thread, so a thread local carries it.
    static ANCHOR_ARM: std::cell::Cell<Option<(gpui::WindowId, Point<Pixels>)>> =
        const { std::cell::Cell::new(None) };
}

/// Make a panel root a window-move surface. The press itself passes
/// through untouched, in capture phase, so the click still lands on the
/// control under it; the move starts only once the pointer clears
/// [`ANCHOR_SLOP`], and the arm dies on release.
fn arm_window_move(root: Div) -> Div {
    root.cursor_grab()
        .capture_any_mouse_down(|event, window, _| {
            if event.button == MouseButton::Left {
                ANCHOR_ARM.set(Some((window.window_handle().window_id(), event.position)));
            }
        })
        .on_mouse_move(|event, window, cx| {
            let Some((id, start)) = ANCHOR_ARM.get() else {
                return;
            };
            // An arm from another window, or one whose release this panel
            // never saw, dies here instead of hijacking an unrelated drag
            // passing over.
            if id != window.window_handle().window_id()
                || event.pressed_button != Some(MouseButton::Left)
            {
                ANCHOR_ARM.set(None);
                return;
            }
            if (event.position.x - start.x).abs() < ANCHOR_SLOP
                && (event.position.y - start.y).abs() < ANCHOR_SLOP
            {
                return;
            }
            ANCHOR_ARM.set(None);
            // The compositor owns the pointer from here; keep this move
            // from doubling as a text-selection drag underneath.
            cx.stop_propagation();
            window.start_window_move();
        })
        .capture_any_mouse_up(|_, _, _| ANCHOR_ARM.set(None))
        .on_mouse_up_out(MouseButton::Left, |_, _, _| ANCHOR_ARM.set(None))
}

/// Build a panel body under its palette override and keep the override
/// active through every element phase. Building under the scope covers
/// the style reads that resolve eagerly (`.bg(palette::x())` runs as the
/// div chain is built); the wrapper element re-enters it for layout,
/// prepaint, and paint, which is when hover styles and canvas paint
/// closures actually read the palette. The theme's frame knobs apply
/// here too, each of them side by side: padding, rounding, and border
/// style the body's root div -
/// the radius must land on the body's own background quad, since gpui
/// content masks stay rectangular and a wrapper's corners would be
/// painted over, and padding on the body keeps the gap in the panel's
/// own background - while margin wraps outside it, so the backdrop
/// shows through that gap. Each knob the theme leaves unset falls back to
/// the app-wide default; an app with no frame set draws none, the look an
/// unthemed panel carried before the knobs were lifted.
pub fn themed(chrome: &PanelChrome, build: impl FnOnce() -> Div) -> AnyElement {
    let theme = &chrome.theme;
    let anchor = chrome.anchor;
    // The panel's own knob wins where it sets one; unset, the panel
    // takes the app-wide default. Zero reads as no knob either way, so
    // an explicit zero over a rounded app default squares this one
    // panel back off, the same as rounding's absence.
    let app = rox_core::settings::app_frame();
    // Every knob comes through `positive`: the app frame is clamped on
    // load, but a panel's own knobs ride a layout dump nobody sanitizes,
    // and a negative inset here would push the panel out of its cell.
    let margin = theme.margin.unwrap_or(app.margin).positive();
    let frame = {
        let padding = theme.padding.unwrap_or(app.padding).positive();
        let rounding = theme.rounding.unwrap_or(app.rounding);
        // Per side, an older config's edge mask folded in on the way.
        let border = theme.border_sides(app.border).positive();
        let font = theme.font.clone();
        move || {
            let mut body = build();
            // The panel's own font layers over the app font the window root
            // cascades in; unset leaves the app font showing through.
            if let Some(font) = font {
                body = body.font_family(font);
            }
            if padding.any() {
                body = body
                    .pt(px(padding.top))
                    .pr(px(padding.right))
                    .pb(px(padding.bottom))
                    .pl(px(padding.left));
            }
            if rounding > 0.0 {
                body = body.rounded(px(rounding));
            }
            if border.any() {
                let widths = &mut body.style().border_widths;
                for (side, width) in [
                    (&mut widths.top, border.top),
                    (&mut widths.right, border.right),
                    (&mut widths.bottom, border.bottom),
                    (&mut widths.left, border.left),
                ] {
                    if width > 0.0 {
                        *side = Some(AbsoluteLength::from(px(width)));
                    }
                }
                body = body.border_color(palette::border());
            }
            // The outer element takes layout and, when the panel is an
            // anchor, the window-move drag. A margin wraps the body in an
            // outer cell; without one the body itself is the root.
            let mut root = if margin.any() {
                div()
                    .size_full()
                    .pt(px(margin.top))
                    .pr(px(margin.right))
                    .pb(px(margin.bottom))
                    .pl(px(margin.left))
                    .child(body)
            } else {
                body
            };
            if anchor {
                root = arm_window_move(root);
            }
            root.into_any_element()
        }
    };
    let scope = theme.scope();
    // A stored 1.0 (or anything that rounds to no change) reads as
    // follow-app, so the wrapper only turns on for a real override.
    let rem_scale = theme
        .font_scale
        .map(|s| s.clamp(palette::PANEL_FONT_SCALE_MIN, palette::PANEL_FONT_SCALE_MAX))
        .filter(|s| (s - 1.0).abs() > 0.001);
    // A surface shader rides the same wrapper: it needs the element's
    // bounds and a paint hook after the body, which is exactly what
    // `Themed` already is.
    let surface = shader::PanelSurface::build(chrome, margin);
    if scope.is_none() && rem_scale.is_none() && surface.is_none() {
        return frame();
    }
    // Build the element under both channels, so a scoped color and a
    // hand-rolled row's `scaled_px` bake in at construction; the wrapper
    // re-applies them through each render phase below.
    let child = panel_env(scope.as_ref(), rem_scale, frame);
    Themed {
        scope,
        rem_scale,
        surface,
        child,
    }
    .into_any_element()
}

/// Run `f` under a panel's palette scope and rem scale, whichever are set.
/// Both the build and the three render phases go through here so a scope
/// color and a `scaled_px` row read the same values every time.
fn panel_env<R>(
    scope: Option<&palette::Scope>,
    rem_scale: Option<f32>,
    f: impl FnOnce() -> R,
) -> R {
    let scaled = move || match rem_scale {
        Some(s) => palette::rem_scaled(s, f),
        None => f(),
    };
    match scope {
        Some(scope) => palette::scoped(scope, scaled),
        None => scaled(),
    }
}

/// The element that carries a panel's palette scope and font scale through
/// the render phases. A pure pass-through for layout; the scope re-applies
/// through the thread-local channel while the font scale rides two rails at
/// once - the window rem (for text and the vendored table, which read it)
/// and the [`palette::rem_scaled`] thread-local (for the hand-rolled rows
/// built without a `Window`). The two stay in step because both derive from
/// the same panel multiplier.
struct Themed {
    scope: Option<palette::Scope>,
    rem_scale: Option<f32>,
    /// The panel's surface shader, recorded after the body paints.
    surface: Option<shader::PanelSurface>,
    child: AnyElement,
}

impl Element for Themed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // Layout is where `.text_xs` and the table's row height resolve the
        // rem, but `with_rem_size` is paint-only, so override the base the
        // way the window root does and put it back after the subtree lays
        // out. No override is active here, so the base is the app size.
        let base = window.rem_size();
        if let Some(scale) = self.rem_scale {
            window.set_rem_size(base * scale);
        }
        let layout_id = panel_env(self.scope.as_ref(), self.rem_scale, || {
            self.child.request_layout(window, cx)
        });
        if self.rem_scale.is_some() {
            window.set_rem_size(base);
        }
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scope = self.scope.as_ref();
        let rem_scale = self.rem_scale;
        let child = &mut self.child;
        // `with_rem_size` no-ops on None, so the unscaled panel pays nothing.
        let rem = rem_scale.map(|scale| window.rem_size() * scale);
        window.with_rem_size(rem, |window| {
            panel_env(scope, rem_scale, || {
                child.prepaint(window, cx);
            });
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scope = self.scope.as_ref();
        let rem_scale = self.rem_scale;
        let child = &mut self.child;
        let rem = rem_scale.map(|scale| window.rem_size() * scale);
        window.with_rem_size(rem, |window| {
            panel_env(scope, rem_scale, || child.paint(window, cx));
        });
        // Post-order: the body is in the scene before the shader records,
        // so a screen pass samples what this panel drew - and a shaded
        // panel nested in a shaded host composes child first, the host
        // reading the finished result.
        if let Some(surface) = &self.surface {
            surface.paint(bounds, window, cx);
        }
    }
}

impl IntoElement for Themed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How far the nudges jump. Longer than the transport panel's ten, since
/// this strip is worked while listening for a change rather than while
/// looking for a spot in a track: fifteen is about how long it takes to
/// hear whether a band or a curve did what was wanted.
const STRIP_SEEK: f64 = 15.0;

/// Back fifteen, play/pause, forward fifteen, random. For the windows that
/// aren't the workspace but still want playback within reach: judging an EQ
/// curve, a signal's band or an output setting means starting and stopping
/// music, and going back to the main window for every pause gets old fast.
/// Four verbs only; the full transport is a panel.
///
/// The nudges take the track buttons' place because of what these windows
/// are for: hearing the same passage again with a knob moved is the loop,
/// and a skip would throw away the passage being judged.
///
/// Nothing says what's playing here. The strip sits centered under a plot
/// in two of its three homes, and a title that grows with the track would
/// shift the buttons out from under the pointer every time one ended.
///
/// The caller has to keep the view awake, since this reads the player every
/// frame and the play/pause face goes stale the moment a track ends on its
/// own. An `cx.observe(&player, ...)` held somewhere does it.
pub fn transport_strip<P: 'static>(
    player: &Entity<Player>,
    library: &Entity<Library>,
    cx: &mut Context<P>,
) -> Div {
    let playing = player.read(cx).is_playing();
    let button = |icon: &'static str,
                  player: Entity<Player>,
                  verb: fn(&mut Player, &mut Context<Player>)| {
        rox_panel_kit::ui::icon_button(icon, false, move |_, _, cx| player.update(cx, verb))
    };
    let random = {
        let player = player.clone();
        let library = library.clone();
        rox_panel_kit::ui::icon_button(icons::DICE, false, move |_, _, cx| {
            player.update(cx, |player, cx| player.play_random(&library, cx));
        })
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(button(icons::SEEK_BACK, player.clone(), |p, _| {
            p.seek_by(-STRIP_SEEK)
        }))
        .child(button(
            if playing { icons::PAUSE } else { icons::PLAY },
            player.clone(),
            |p, _| p.toggle_pause(),
        ))
        .child(button(icons::SEEK_FORWARD, player.clone(), |p, _| {
            p.seek_by(STRIP_SEEK)
        }))
        // Random draws through the scope the continuation system tracks, so
        // it stays inside a playlist the way the transport panel's does.
        .child(random)
}

/// A panel in a window of its own: the panel view, full-size, on the same
/// base styling the workspace root applies. Right-click serves the panel's
/// own menu, the one its tab would drop down in the dock.
struct PopoutHost {
    panel_view: Arc<dyn PanelView>,
    state: AppState,
    /// This window's slice of the backdrop: what it painted last, for
    /// retiring the texture on a new bake.
    backdrop: WindowBackdrop,
    /// The open right-click menu: its anchor position, the menu, and the
    /// dismiss subscription that clears it.
    context_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Fallback focus so the Workspace-scoped playback bindings keep a
    /// dispatch path in this window even before the hosted panel takes
    /// focus. Mirrors the main workspace's fallback focus.
    focus: FocusHandle,
    /// The window this host fills, so closing it can drop the window's entry
    /// from [`PANEL_WINDOWS`].
    window_id: u64,
    _backdrop_changed: Subscription,
}

impl Drop for PopoutHost {
    fn drop(&mut self) {
        PANEL_WINDOWS.write().unwrap().remove(&self.window_id);
    }
}

impl PopoutHost {
    /// Open the right-click menu: the panel's own dropdown, everything its
    /// tab would offer in the dock - its content entries, Save As Preset,
    /// Rename, Panel Settings - ending in Dock Back, which moves the panel
    /// into the workspace's newest live tab group and closes this window.
    /// Cross-window drags can't work (a held button pins pointer events to
    /// its window, and Wayland hides window positions), so that row is the
    /// way home; a window the panel was opened straight into leaves it out.
    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.panel_view.clone();
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            panel.dropdown_menu(menu, window, cx)
        });
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((position, menu, subscription));
        cx.notify();
    }
}

impl Render for PopoutHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A popped-out window shares its parent's player, so it renders
        // under that playback's tint, and claims the widget theme while it
        // holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                // Same Workspace context and playback actions as the main
                // window, so space and the seek arrows work in a popout too.
                // The panel's own SearchInput context still carves the keys
                // back for its search box.
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
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // A panel that serves its own content menu already ends it
                // with the same panel tail this one would show, so the
                // window's own right-click would only stack a second menu on
                // top. Install it only for panels with no content menu.
                .when(!self.panel_view.content_context_menu(cx), |body| {
                    body.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.open_menu(event.position, window, cx);
                        }),
                    )
                })
                // The backdrop paints first, under the panel; how much shows
                // through is the surfaces' call (ADR 10's strength scalar).
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(self.panel_view.view())
                // Same overlay structure as the dock's context menu: an
                // occluding layer swallows the dismissing click, the anchored
                // child pins the menu to the pointer.
                .when_some(self.context_menu.as_ref(), |this, (position, menu, _)| {
                    this.child(
                        deferred(
                            anchored().child(
                                div()
                                    .w(window.bounds().size.width)
                                    .h(window.bounds().size.height)
                                    .occlude()
                                    .child(
                                        anchored()
                                            .position(*position)
                                            .snap_to_window_with_margin(px(8.))
                                            .child(menu.clone()),
                                    ),
                            ),
                        )
                        .with_priority(1),
                    )
                })
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    /// A panel config the way every real one is shaped: its own fields
    /// beside the flattened chrome.
    #[derive(Default, Serialize, Deserialize)]
    struct StubConfig {
        #[serde(default)]
        tile: f32,
        #[serde(flatten)]
        chrome: PanelChrome,
    }

    #[test]
    fn chrome_round_trips_with_a_shader() {
        let mut chrome = PanelChrome {
            title: Some("Wall".to_string()),
            locked: true,
            ..PanelChrome::default()
        };
        chrome.shader = Some(PanelShader {
            enabled: true,
            source: "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }".to_string(),
            name: None,
            path: Some("/tmp/smudge.wgsl".into()),
            routes: vec![rox_viz::signal::Route {
                enabled: true,
                signal: 3,
                target: shader::slot_target(1),
                from: 0.0,
                to: 1.0,
            }],
            manual: vec![(4, 0.25)],
            run_when_idle: true,
        });
        let config = StubConfig { tile: 96.0, chrome };

        let dumped = serde_json::to_value(&config).expect("dump");
        let read: StubConfig = serde_json::from_value(dumped).expect("read back");

        assert_eq!(read.tile, 96.0);
        assert_eq!(read.chrome.title.as_deref(), Some("Wall"));
        let shader = read
            .chrome
            .shader
            .expect("the shader survives the round trip");
        assert!(shader.enabled);
        assert!(shader.run_when_idle);
        assert!(shader.source.contains("fs_user"));
        assert_eq!(shader.routes.len(), 1);
        assert_eq!(shader.routes[0].target, "slot1");
        // The hand-set knobs ride the dump beside the routes, so a panel
        // wearing a tuned shader comes back tuned.
        assert_eq!(shader::manual_value(&shader.manual, 4), Some(0.25));
    }

    #[test]
    fn chrome_without_a_shader_writes_no_field() {
        let config = StubConfig::default();
        let dumped = serde_json::to_value(&config).expect("dump");
        assert!(
            dumped.get("shader").is_none(),
            "an unshaded panel shouldn't grow a shader key: {dumped}"
        );
    }

    #[test]
    fn an_old_dump_loads_clean() {
        // A layout written before panel shaders existed: chrome fields and
        // the panel's own, no shader key anywhere.
        let dumped = serde_json::json!({
            "tile": 120.0,
            "title": "Grid",
            "locked": true,
            "max_width": 400.0,
        });
        let read: StubConfig = serde_json::from_value(dumped).expect("old dumps still load");
        assert_eq!(read.tile, 120.0);
        assert_eq!(read.chrome.title.as_deref(), Some("Grid"));
        assert!(read.chrome.locked);
        assert_eq!(read.chrome.max_width, Some(400.0));
        assert!(read.chrome.shader.is_none());
    }

    /// A min set above the max is a settings file the user typed, not a
    /// state the dock can lay out. The min wins on both axes, and the
    /// panel's built-in floor still beats a min under it.
    #[test]
    fn a_min_over_the_max_raises_the_cap() {
        let floor = gpui::size(px(120.), px(80.));
        let chrome = PanelChrome {
            min_width: Some(500.),
            max_width: Some(300.),
            min_height: Some(400.),
            max_height: Some(200.),
            ..PanelChrome::default()
        };
        let min = chrome_min_size(&chrome, floor);
        let max = chrome_max_size(&chrome, floor);
        assert_eq!(min.width, px(500.));
        assert_eq!(min.height, px(400.));
        assert_eq!(max.width, px(500.));
        assert_eq!(max.height, px(400.));

        // A min under the panel's floor lifts to the floor, and the cap
        // follows it there rather than sitting below.
        let squeezed = PanelChrome {
            min_width: Some(40.),
            max_width: Some(60.),
            ..PanelChrome::default()
        };
        assert_eq!(chrome_min_size(&squeezed, floor).width, px(120.));
        assert_eq!(chrome_max_size(&squeezed, floor).width, px(120.));

        // A sane pair is left exactly as written, and an unset cap stays
        // unbounded however high the min goes.
        let sane = PanelChrome {
            min_width: Some(200.),
            max_width: Some(600.),
            min_height: Some(300.),
            ..PanelChrome::default()
        };
        assert_eq!(chrome_max_size(&sane, floor).width, px(600.));
        assert_eq!(chrome_max_size(&sane, floor).height, Pixels::MAX);
    }
}
