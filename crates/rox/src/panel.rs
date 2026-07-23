//! The app's own panel layer per ADR 7: the dock, tabs, splits, and resize
//! come from gpui-component, and the two behaviors it doesn't give us live
//! here. Panels are views over the shared entities in [`AppState`], so a
//! duplicate is a second view with its own config over the same state, and a
//! popped-out panel is the same entity rehosted in its own OS window, no
//! cross-window messaging needed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    anchored, canvas, deferred, div, fill, point, prelude::*, px, relative, size, svg,
    AbsoluteLength, Along, AnyElement, App, Axis, Bounds, Context, DismissEvent, Div, Element,
    Entity, FocusHandle, Focusable as _, GlobalElementId, InspectorElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollHandle,
    SharedString, Size, Stateful, Subscription, TitlebarOptions, UniformListScrollHandle,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::button::Button;
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{h_flex, Icon, IconName, Root, Sizable};
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::history::History;
use crate::lastfm::Scrobbler;
use crate::panels::library::Library;
use crate::player::{fmt_time, Player};
use crate::query::shared_query::SharedQuery;
use crate::selection::Selection;
use crate::thumbs::Thumbs;
use crate::workspace::{SeekBackward, SeekForward, TogglePlayback};

mod gesture;
pub use gesture::*;

mod tracked_load;
pub use tracked_load::TrackedImage;

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
    /// The last.fm scrobbler over this workspace's player; also where the
    /// live scrobble config lives, for the panels' threshold markers.
    pub scrobbler: Entity<Scrobbler>,
    /// The listen recorder riding the scrobbler's listen signal; history
    /// views subscribe to it for the refresh when an event lands.
    pub history: Entity<History>,
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

/// The flat icon button the transport panels share so the button style
/// never forks: the icon alone at rest, a soft pill behind it on hover.
/// Icon paths come from [`crate::assets::icons`].
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: Rgba,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    icon_control_sized(icon, px(16.), color, on_click, cx)
}

/// [`icon_control`] with the icon size exposed, for spots like the menubar
/// where the transport-scale glyph reads too heavy.
pub fn icon_control_sized<V: 'static>(
    icon: &'static str,
    size: Pixels,
    color: Rgba,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> impl IntoElement {
    div()
        .p(tokens::ICON_PAD)
        .rounded(tokens::RADIUS)
        .hover(|d| d.bg(palette::bg_control()))
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_click(this, cx)),
        )
        .child(svg().path(icon).size(size).text_color(color))
}

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

/// A panel's tab and title text: the rename when one is set, the built-in
/// name otherwise.
pub fn title_text(custom: Option<&str>, default: &'static str) -> SharedString {
    match custom {
        Some(name) => SharedString::from(name.to_owned()),
        None => default.into(),
    }
}

/// Title-case a panel's built-in name for display. The name is a
/// serialized identifier (lowercase, space separated); tab and window
/// titles want it capitalized. No panel name contains "rox" or an
/// acronym, so a plain per-word capitalize is right here.
pub fn display_name(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
/// closing the OS window is the close. Pinned panels keep the dock menus'
/// guard and the click no-ops.
pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
) -> PopupMenu {
    let pop_panel = panel.clone();
    let pop_tabs = tab_panel.clone();
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
    let Some(tabs) = tab_panel else {
        return menu;
    };
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Close")
            .icon(Icon::default().path(icons::CLOSE))
            .on_click(move |_, window, cx| {
                let Some(tabs) = tabs.upgrade() else {
                    return;
                };
                if panel.read(cx).locked(cx) {
                    return;
                }
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
/// A popped-out panel has no tab strip to add to, so the entry no-ops.
pub fn duplicate_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    make: impl Fn(&Entity<P>, &mut Window, &mut Context<P>) -> P + 'static,
) -> PopupMenu {
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

/// A checkable flyout row whose tick tracks the live panel value instead of
/// one baked in when the menu was built. Pair it with [`follow_panel`] in the
/// submenu builder: the flyout re-renders on the click, this row re-reads the
/// value, and the tick swaps in place.
///
/// Plain `.checked(..)` rows go stale in an open flyout, our hand-built
/// submenus never dismiss on click (they carry no link back to the root menu,
/// so there is no reopen to rebuild them), so a static tick would sit wrong
/// until the whole menu is closed and reopened.
///
/// `is_on` reads the state each render, `toggle` flips it. A left `icon`
/// keeps the row looking like its plain sibling, with the tick pushed to the
/// right so the icon is not replaced. Without an icon the tick takes the left
/// slot, matching the default check side.
pub fn check_row<P: 'static>(
    label: impl Into<SharedString>,
    icon: Option<&'static str>,
    is_on: impl Fn(&P) -> bool + 'static,
    toggle: impl Fn(&mut P, &mut Context<P>) + 'static,
    panel: &Entity<P>,
) -> PopupMenuItem {
    let label: SharedString = label.into();
    let read = panel.clone();
    let weak = panel.downgrade();
    PopupMenuItem::element(move |_, cx| {
        let on = is_on(read.read(cx));
        if let Some(icon) = icon {
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_x_1()
                        .items_center()
                        .child(Icon::default().path(icon).xsmall())
                        .child(label.clone()),
                )
                .when(on, |row| row.child(Icon::new(IconName::Check).xsmall()))
        } else {
            h_flex()
                .gap_x_1()
                .items_center()
                .child(if on {
                    Icon::new(IconName::Check).xsmall().into_any_element()
                } else {
                    Icon::empty().xsmall().into_any_element()
                })
                .child(label.clone())
        }
    })
    .on_click(move |_, _, cx| {
        let Some(this) = weak.upgrade() else { return };
        this.update(cx, |this, cx| {
            toggle(this, cx);
            cx.notify();
        });
    })
}

/// Re-render an open flyout whenever `panel` changes, so its [`check_row`]s
/// pick up the flip without the menu closing. Call once in the submenu
/// builder, where `cx` is the submenu's own context.
pub fn follow_panel<P: 'static>(panel: &Entity<P>, cx: &mut Context<PopupMenu>) {
    cx.observe(panel, |_, _, cx| cx.notify()).detach();
}

/// Resolve track ids to paths and hand them to the player: after the playing
/// track when `next`, at the tail otherwise. Shared by the context-menu
/// actions across every song surface.
pub fn queue_tracks(state: &AppState, ids: &[i64], next: bool, cx: &mut App) {
    let paths = match state.library.read(cx).paths_for(ids) {
        Ok(paths) if !paths.is_empty() => paths,
        _ => return,
    };
    state.player.update(cx, |player, cx| {
        if next {
            player.play_next(paths, cx);
        } else {
            player.enqueue(paths, cx);
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
                    crate::playlist_create::open(new_state.clone(), new_ids.clone(), cx);
                }),
        );
        let playlists = playlist_state.library.read(cx).playlists();
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
                    crate::tags::editor::open(tag_state.clone(), tag_ids.clone(), cx);
                }),
        )
        // Covers get their own window: the tag editor edits text per
        // track, this stamps one image across the selection.
        .item(
            PopupMenuItem::new("Edit Cover Art...")
                .icon(Icon::default().path(icons::IMAGE))
                .on_click(move |_, _, cx| {
                    crate::cover::editor::open(cover_state.clone(), ids.clone(), cx);
                }),
        );
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
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // The Wayland backend ignores the creation-time titlebar title;
        // only set_window_title reaches the compositor.
        window.set_window_title(&title);
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
        app_id: Some(crate::APP_ID.into()),
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
}

/// The panel's size cap as a [`Size`], reading the chrome's optional
/// width/height limits over `floor` (the panel's minimum, so a cap can
/// never drop below what the panel needs). An unset axis stays unbounded.
/// Every panel returns this from its `Panel::max_size`, so the cap is a
/// generic panel setting rather than a per-panel opt-in.
pub fn chrome_max_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let axis = |cap: Option<f32>, floor: Pixels| match cap {
        Some(px_value) => px(px_value).max(floor),
        None => Pixels::MAX,
    };
    gpui::size(
        axis(chrome.max_width, floor.width),
        axis(chrome.max_height, floor.height),
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
/// (see [`crate::panel_settings`]): the panel's own pages of control
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

/// Build a panel body under its palette override and keep the override
/// active through every element phase. Building under the scope covers
/// the style reads that resolve eagerly (`.bg(palette::x())` runs as the
/// div chain is built); the wrapper element re-enters it for layout,
/// prepaint, and paint, which is when hover styles and canvas paint
/// closures actually read the palette. The theme's frame knobs apply
/// here too: padding, rounding, and border style the body's root div -
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
    let frame = {
        // The panel's own knob wins where it sets one; unset, the panel
        // takes the app-wide default. Zero reads as no knob either way, so
        // an explicit zero over a rounded app default squares this one
        // panel back off, the same as rounding's absence.
        let app = crate::settings::app_frame();
        let margin = theme.margin.unwrap_or(app.margin);
        let padding = theme.padding.unwrap_or(app.padding);
        let rounding = theme.rounding.unwrap_or(app.rounding);
        let border = theme.border.unwrap_or(app.border);
        let font = theme.font.clone();
        move || {
            let mut body = build();
            // The panel's own font layers over the app font the window root
            // cascades in; unset leaves the app font showing through.
            if let Some(font) = font {
                body = body.font_family(font);
            }
            if padding > 0.0 {
                body = body.p(px(padding));
            }
            if rounding > 0.0 {
                body = body.rounded(px(rounding));
            }
            if border > 0.0 {
                let width: AbsoluteLength = px(border).into();
                let widths = &mut body.style().border_widths;
                widths.top = Some(width);
                widths.right = Some(width);
                widths.bottom = Some(width);
                widths.left = Some(width);
                body = body.border_color(palette::border());
            }
            // The outer element takes layout and, when the panel is an
            // anchor, the window-move drag. A margin wraps the body in an
            // outer cell; without one the body itself is the root.
            let mut root = if margin > 0.0 {
                div().size_full().p(px(margin)).child(body)
            } else {
                body
            };
            if anchor {
                root = root
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move());
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
    if scope.is_none() && rem_scale.is_none() {
        return frame();
    }
    // Build the element under both channels, so a scoped color and a
    // hand-rolled row's `scaled_px` bake in at construction; the wrapper
    // re-applies them through each render phase below.
    let child = panel_env(scope.as_ref(), rem_scale, frame);
    Themed {
        scope,
        rem_scale,
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
        _bounds: Bounds<Pixels>,
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
    }
}

impl IntoElement for Themed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Wraps a window's whole body in its player's art tint, the mirror of
/// [`Themed`] one level up: the palette accessors answer from the tint
/// while the tree is built and again through every paint phase, so a
/// window's panels and canvases read its own playback's colors. Built with
/// [`window_body`], which snapshots the tint and runs the body inside it.
pub struct WindowTint {
    tint: palette::Tint,
    child: AnyElement,
}

/// Build a window body under its player's art tint. The body closure runs
/// with the tint pushed so render-time color reads see it, and the tint
/// rides along into the paint phases through the returned element.
pub fn window_body(player: gpui::EntityId, body: impl FnOnce() -> AnyElement) -> WindowTint {
    let tint = palette::window_tint(player);
    let child = palette::tinted(tint, body);
    WindowTint { tint, child }
}

impl Element for WindowTint {
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
        let layout_id = palette::tinted(self.tint, || self.child.request_layout(window, cx));
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
        palette::tinted(self.tint, || {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::tinted(self.tint, || self.child.paint(window, cx));
    }
}

impl IntoElement for WindowTint {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// One labeled row of a customize window: the setting's name and its
/// control on one line, an optional dimmed description wrapping below.
pub fn setting_row(
    label: &'static str,
    description: Option<&'static str>,
    control: impl IntoElement,
) -> Div {
    setting_row_dyn(label, description.map(SharedString::from), control)
}

/// [`setting_row`] with a built description, for the rare row whose note
/// carries live numbers rather than fixed copy.
pub fn setting_row_dyn(
    label: &'static str,
    description: Option<SharedString>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label)
                .child(div().flex_none().child(control)),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
}

/// A labeled block of a customize window: like [`setting_row`] but the
/// control spans the full width below the description instead of sitting
/// inline. Wrapping controls need this - the row's control slot is
/// content-sized, and a wrap container without a definite width collapses
/// to one item per line. An optional trailing control rides the label
/// row's right edge, where a section's reset button lives.
pub fn setting_block(
    label: &'static str,
    description: Option<&'static str>,
    trailing: Option<AnyElement>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label)
                .when_some(trailing, |d, trailing| {
                    d.child(div().flex_none().child(trailing))
                }),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
        .child(div().mt(tokens::SPACE_XS).child(control))
}

/// The settings-page sliders' strip width and the readout beside them.
pub const SLIDER_W: Pixels = px(150.);
pub const READOUT_W: Pixels = px(60.);

/// One scalar's slider row: the shared slider chrome over a scrub strip,
/// applying the strip fraction live on click and drag, the readout riding
/// alongside. Callers map the fraction into their own range and format
/// their own readout.
pub fn value_slider<P: 'static>(
    scrub: &ScrubState,
    fraction: f32,
    readout: String,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let entity = cx.entity();
    let strip = div()
        .w(SLIDER_W)
        .h(tokens::CONTROL_H)
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let scrub = scrub.clone();
                let apply = apply.clone();
                move |this: &mut P, event: &MouseDownEvent, _, cx| {
                    scrub.begin();
                    if let Some(fraction) = scrub.fraction(event.position.x) {
                        apply(this, fraction, cx);
                    }
                    cx.notify();
                }
            }),
        )
        .child(
            canvas(
                {
                    let scrub = scrub.clone();
                    move |bounds, _, _| scrub.set_bounds(bounds)
                },
                {
                    let scrub = scrub.clone();
                    move |bounds, _, window, _| {
                        paint_slider(fraction, false, bounds, window);
                        scrub_on_paint(&scrub, window, {
                            let entity = entity.clone();
                            let apply = apply.clone();
                            move |fraction, cx| {
                                entity.update(cx, |this, cx| apply(this, fraction, cx));
                            }
                        });
                    }
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(strip)
        .child(
            div()
                .w(READOUT_W)
                .flex_none()
                .text_right()
                .text_color(palette::text_muted())
                .child(readout),
        )
}

/// The switch pill and knob without any interaction, shared by [`toggle`] and
/// [`toggle_locked`].
fn toggle_track(on: bool) -> Div {
    div()
        .w(px(34.))
        .h(px(18.))
        .flex_none()
        .rounded_full()
        .bg(palette::bg_control())
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .p(px(2.))
        .child(div().size(px(14.)).rounded_full().bg(if on {
            palette::accent()
        } else {
            palette::text_faint()
        }))
}

/// An on/off switch: a pill track, the knob in the accent on the far side
/// while on.
pub fn toggle<P: 'static>(
    on: bool,
    on_change: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    toggle_track(on).cursor_pointer().on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _, _, cx| on_change(this, !on, cx)),
    )
}

/// A [`toggle`] the user cannot flip: dimmed and inert, the same shape as the
/// live switch. For a setting the app is holding at a value, like the watch
/// switch a library grows too large to arm.
pub fn toggle_locked(on: bool) -> Div {
    toggle_track(on).opacity(0.5)
}

/// How long a run of keystrokes stays one type-ahead phrase: a pause past
/// this starts the buffer over. Shared by every panel that jumps by prefix.
pub const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// Grow or restart a type-ahead buffer for the keystroke `text`: within the
/// window since the last stroke the letters build one phrase, past it the
/// phrase starts fresh. Stamps `at` with now and returns whether the phrase
/// grew, which the callers use to decide the match re-tests the current row
/// or steps past it. The prefix match and the scroll that follow stay per
/// panel, since the list widget and what a row's text is differ.
pub fn type_ahead_grow(buffer: &mut String, at: &mut Option<Instant>, text: String) -> bool {
    let now = Instant::now();
    let grown = at.is_some_and(|last| now.duration_since(last) < TYPE_AHEAD);
    if grown {
        buffer.push_str(&text);
    } else {
        *buffer = text;
    }
    *at = Some(now);
    grown
}

/// The shared "tracking" section for a panel's Behavior page: the
/// follow-playing toggle and, while it is on, the smooth-scrolling toggle,
/// under one header so the library, the grids, and the art shelf all read
/// the same. The wording of what it follows (a row, an album, the center)
/// differs per panel, so both descriptions are passed in; the toggles carry
/// each panel's own follow and glide handlers.
#[allow(clippy::too_many_arguments)]
pub fn tracking_section<P: 'static>(
    follow: bool,
    follow_desc: &'static str,
    on_follow: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    resume: bool,
    resume_desc: &'static str,
    on_resume: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    smooth: bool,
    smooth_desc: &'static str,
    on_smooth: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> AnyElement {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_MD)
        .child(setting_row(
            "Follow Playing",
            Some(follow_desc),
            toggle(follow, on_follow, cx),
        ))
        .child(setting_row(
            "Resume When Idle",
            Some(resume_desc),
            toggle(resume, on_resume, cx),
        ));
    // Both the follow and the resume ride the same glide, so the motion
    // toggle earns its place the moment either is on.
    if follow || resume {
        body = body.child(setting_row(
            "Smooth Scrolling",
            Some(smooth_desc),
            toggle(smooth, on_smooth, cx),
        ));
    }
    crate::settings::ui::section("Tracking", None, body).into_any_element()
}

/// A font-family picker: a small dropdown labeled with the current
/// choice, its menu the installed families over a Default that clears the
/// override back to the app font. `current` is the panel's stored family,
/// None meaning inherit; `apply` stores the pick. Shared so any panel that
/// carries a font override draws the same control - the lyrics panel's
/// typeface knob is the first.
pub fn font_picker<P: 'static>(
    id: &'static str,
    current: Option<String>,
    apply: impl Fn(&mut P, Option<String>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
    let label: SharedString = current
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| "Default".into());
    // The installed families don't change over a session, so enumerate and sort
    // them once and share the list. This runs on every settings render, slider
    // scrubs included, where re-listing and re-sorting every font each frame was
    // pure waste.
    static FONTS: OnceLock<Arc<Vec<SharedString>>> = OnceLock::new();
    let fonts = FONTS
        .get_or_init(|| {
            let mut fonts = cx.text_system().all_font_names();
            fonts.sort();
            fonts.dedup();
            Arc::new(fonts.into_iter().map(SharedString::from).collect())
        })
        .clone();
    let weak = cx.entity().downgrade();
    Button::new(id)
        .label(label)
        .small()
        .outline()
        .dropdown_menu(move |menu, _, _| {
            let clear = weak.clone();
            let clear_apply = apply.clone();
            let mut menu = menu.item(
                PopupMenuItem::new("Default")
                    .checked(current.is_none())
                    .on_click(move |_, _, cx| {
                        if let Some(this) = clear.upgrade() {
                            let apply = clear_apply.clone();
                            this.update(cx, |this, cx| apply(this, None, cx));
                        }
                    }),
            );
            for name in fonts.iter() {
                let name = name.clone();
                let checked = current.as_deref() == Some(name.as_ref());
                let pick = weak.clone();
                let apply = apply.clone();
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        let name = name.to_string();
                        let apply = apply.clone();
                        if let Some(this) = pick.upgrade() {
                            this.update(cx, |this, cx| apply(this, Some(name), cx));
                        }
                    },
                ));
            }
            menu
        })
}

/// The chrome shared by the segmented pickers: a joined group of segments,
/// the picked one filled with the accent, hairline gaps between the rest.
fn segments<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    render: impl Fn(&'static str, bool) -> AnyElement,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let last = options.len().saturating_sub(1);
    let mut group = div().flex().flex_row().flex_none().items_center();
    for (i, (key, value)) in options.iter().enumerate() {
        let value = *value;
        let picked = value == current;
        let on_pick = on_pick.clone();
        group = group.child(
            div()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .when(i > 0, |d| d.ml(px(1.)))
                .when(i == 0, |d| d.rounded_l(tokens::RADIUS))
                .when(i == last, |d| d.rounded_r(tokens::RADIUS))
                .bg(if picked {
                    palette::accent()
                } else {
                    palette::bg_control()
                })
                .when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                )
                .child(render(key, picked)),
        );
    }
    group
}

/// A segmented picker of exclusive choices, labeled with text.
pub fn choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |label, picked| {
            div()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .child(label)
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// A segmented picker of exclusive choices, labeled with icons; each option
/// pairs an icon path from [`crate::assets::icons`] with its value.
pub fn icon_choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        current,
        |icon, picked| {
            svg()
                .path(icon)
                .size_4()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// Where a panel's content sits horizontally, the cross-panel
/// customization knob.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Apply an alignment along a row's main axis.
pub fn justify(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::Right => d.justify_end(),
    }
}

/// Apply an alignment along the cross axis, so a column's children sit
/// left, center, or right the way `justify` places a row's.
pub fn items(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.items_start(),
        Align::Center => d.items_center(),
        Align::Right => d.items_end(),
    }
}

/// The alignment setting row the panels' customize windows share.
pub fn align_row<P: 'static>(
    current: Align,
    on_pick: impl Fn(&mut P, Align, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        "Alignment",
        Some("Where the content sits when the panel has room to spare"),
        icon_choices(
            &[
                (icons::ALIGN_LEFT, Align::Left),
                (icons::ALIGN_CENTER, Align::Center),
                (icons::ALIGN_RIGHT, Align::Right),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}

/// A popped-out panel's window content: the moved panel view, full-size, on
/// the same base styling the workspace root applies. Right-click offers the
/// way back into the dock.
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
    _backdrop_changed: Subscription,
}

impl PopoutHost {
    /// Open the right-click menu. Dock Back moves the panel into the newest
    /// live tab group of the workspace and closes this window; cross-window
    /// drags can't work (a held button pins pointer events to its window,
    /// and Wayland hides window positions), so this is the way home.
    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.panel_view.clone();
        let hosts = self.state.tab_hosts.clone();
        let dockable = hosts.read(cx).last_live(cx).is_some();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.item(
                PopupMenuItem::new("Dock Back")
                    .disabled(!dockable)
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
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.open_menu(event.position, window, cx);
                    }),
                )
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
