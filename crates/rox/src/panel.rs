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
    anchored, canvas, deferred, div, fill, linear_color_stop, linear_gradient, point, prelude::*,
    px, relative, size, svg, AbsoluteLength, Action, Along, AnyElement, App, Axis, Bounds, Context,
    DismissEvent, Div, Element, Entity, FocusHandle, Focusable as _, GlobalElementId,
    InspectorElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Rgba, ScrollHandle, SharedString, Size, Stateful, Subscription, TitlebarOptions,
    UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, Disableable, Icon, IconName, Root, Sizable};
use rox_dock::{Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::history::History;
use crate::integrations::discord::DiscordPresence;
use crate::lastfm::Scrobbler;
use crate::panels::library::Library;
use crate::player::{fmt_time, FadeView, Player};
use crate::portraits::Portraits;
use crate::query::shared_query::SharedQuery;
use crate::selection::Selection;
use crate::thumbs::Thumbs;
use crate::workspace::{workspace_for_window, SeekBackward, SeekForward, TogglePlayback};

mod arrange;
pub use arrange::*;

mod gesture;
pub use gesture::*;

pub mod shader;
pub use shader::PanelShader;

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

/// What a control's hover tooltip says, and the identity gpui parks its
/// timing under. Every [`icon_control`] takes one: a glyph on its own says
/// nothing to anyone who doesn't already know the app, so a new button
/// can't ship without naming what it does.
///
/// gpui keeps the hover timer in element state, which only elements with
/// an id get, so a tipped control needs an id too. A static label is its
/// own id. Anything whose words read live (the loop button's mode, a
/// per-row play button) takes [`Tip::keyed`] instead, so the id stays put
/// while the text moves and two rows never share one timer.
pub struct Tip {
    id: gpui::ElementId,
    text: SharedString,
    action: Option<(Box<dyn Action>, Option<&'static str>)>,
}

impl Tip {
    /// A tip whose words change, under an id that doesn't.
    pub fn keyed(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            action: None,
        }
    }

    /// Trail the shortcut that does the same thing. `context` is the key
    /// context the binding resolves in (`Workspace`), not the predicate it
    /// was registered with, which parses as a context and finds nothing.
    pub fn action(mut self, action: &dyn Action, context: Option<&'static str>) -> Self {
        self.action = Some((action.boxed_clone(), context));
        self
    }

    /// Hang the tip off a control that builds itself, for the buttons the
    /// shared [`icon_control`] has no room for.
    pub fn apply(self, control: Div) -> Stateful<Div> {
        let Self { id, text, action } = self;
        control.id(id).tooltip(move |window, cx| {
            let mut tip = Tooltip::new(text.clone());
            if let Some((action, context)) = action.as_ref() {
                tip = tip.action(action.as_ref(), *context);
            }
            tip.build(window, cx)
        })
    }
}

impl From<&'static str> for Tip {
    fn from(text: &'static str) -> Self {
        Self::keyed(text, text)
    }
}

/// The flat icon button the transport panels share so the button style
/// never forks: the icon alone at rest, a soft pill behind it on hover,
/// and a [`Tip`] naming it once the pointer settles. Icon paths come from
/// [`crate::assets::icons`].
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: Rgba,
    tip: impl Into<Tip>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    icon_control_sized(icon, px(16.), color, tip, on_click, cx)
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

/// [`icon_control`] with the icon size exposed, for spots like the menubar
/// where the transport-scale glyph reads too heavy.
pub fn icon_control_sized<V: 'static>(
    icon: &'static str,
    size: Pixels,
    color: Rgba,
    tip: impl Into<Tip>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    tip.into().apply(
        div()
            .p(tokens::ICON_PAD)
            .rounded(tokens::RADIUS)
            .hover(|d| d.bg(palette::bg_control()))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            )
            .child(svg().path(icon).size(size).text_color(color)),
    )
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
) -> PopupMenu {
    // No tab strip means the panel is popped out into its own window; there
    // the item that belongs here is the way home, not another Pop Out.
    let Some(tabs) = tab_panel.clone() else {
        return dock_back_item(menu, Arc::new(panel.clone()), state);
    };
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
                    let Some(ws) = workspace_for_window(window, cx).and_then(|ws| ws.upgrade())
                    else {
                        return;
                    };
                    let panel: Arc<dyn PanelView> = Arc::new(panel.clone());
                    let tabs = tabs.clone();
                    ws.update(cx, |ws, cx| {
                        ws.confirm_close_locked(panel, tabs, window, cx);
                    });
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
/// `is_on` reads the state each render, `toggle` flips it. An `icon` rides on
/// the item rather than inside our element, so it lands in the same reserved
/// left slot the plain rows use and the row lines up with its neighbours;
/// the tick then sits on the right, matching `check_side(Side::Right)`.
/// Without an icon the tick takes the left slot, matching the default check
/// side, which is the shape flyouts of bare toggles use.
///
/// Drawing the icon inside the element instead would double-indent the row:
/// the menu reserves a left slot as soon as any item carries an icon, so a
/// self-drawn icon sits one slot further in than everything around it.
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
    let has_icon = icon.is_some();
    let item = PopupMenuItem::element(move |_, cx| {
        let on = is_on(read.read(cx));
        if has_icon {
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(label.clone())
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
    });
    let item = match icon {
        Some(icon) => item.icon(Icon::default().path(icon)),
        None => item,
    };
    item.on_click(move |_, _, cx| {
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
        // A popped-out panel keeps its surface shader, so this window needs
        // the hub and player its slots read from.
        shader::note_window(window, &state, cx);
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
    // The panel's own knob wins where it sets one; unset, the panel
    // takes the app-wide default. Zero reads as no knob either way, so
    // an explicit zero over a rounded app default squares this one
    // panel back off, the same as rounding's absence.
    let app = crate::settings::app_frame();
    let margin = theme.margin.unwrap_or(app.margin);
    let frame = {
        let padding = theme.padding.unwrap_or(app.padding);
        let rounding = theme.rounding.unwrap_or(app.rounding);
        let border = theme.border.unwrap_or(app.border);
        // The edge mask is the panel's alone; there is no app-wide one to
        // inherit, so unset just means all four sides.
        let edges = theme.border_edges.unwrap_or(palette::BorderEdges::ALL);
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
            if border > 0.0 && edges.any() {
                let width: AbsoluteLength = px(border).into();
                let widths = &mut body.style().border_widths;
                if edges.top {
                    widths.top = Some(width);
                }
                if edges.right {
                    widths.right = Some(width);
                }
                if edges.bottom {
                    widths.bottom = Some(width);
                }
                if edges.left {
                    widths.left = Some(width);
                }
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
/// What a [`banner`] is telling you, which picks its color and its face.
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Just so you know. The state is fine and unremarkable.
    Info,
    /// The good outcome, called out because it's the one worth confirming.
    Good,
    /// Something is standing in for what was asked.
    Warn,
    /// Something failed.
    Bad,
}

impl Tone {
    fn color(self) -> Rgba {
        match self {
            Tone::Info => palette::text_muted(),
            Tone::Good => palette::tone_good(),
            Tone::Warn => palette::tone_warn(),
            Tone::Bad => palette::tone_bad(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tone::Info => icons::INFO,
            Tone::Good => icons::CHECK,
            Tone::Warn | Tone::Bad => icons::ALERT,
        }
    }
}

/// A callout: a tinted box with a rule down its edge, a face, a headline,
/// and however many lines of detail under it. For state a row can't carry,
/// where what happened needs more than a value and the difference between
/// fine and not fine should be visible before anything is read.
///
/// The tint is the tone at low alpha over whatever the surface already is,
/// so it reads on both themes and under the art wash without a second set
/// of colors.
pub fn banner(tone: Tone, headline: impl Into<SharedString>, lines: Vec<SharedString>) -> Div {
    banner_shaped(tone, headline, lines, false)
}

/// The same callout, flowing: the reasons ride beside the headline while
/// there's width for them and drop under it when there isn't. For a panel
/// that has to earn its height, where a block stacked three lines deep to
/// say two short things wastes the strip it's parked in.
pub fn banner_flow(tone: Tone, headline: impl Into<SharedString>, lines: Vec<SharedString>) -> Div {
    banner_shaped(tone, headline, lines, true)
}

fn banner_shaped(
    tone: Tone,
    headline: impl Into<SharedString>,
    lines: Vec<SharedString>,
    flow: bool,
) -> Div {
    let color = tone.color();
    // The face rides the headline's own row rather than the whole block, so
    // it centers against that one line however many lines follow and however
    // far they wrap. Hanging it off the block instead left it floating high
    // the moment a reason wrapped to two lines.
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        // Only where the row it sits in has a width to shrink against. In a
        // block sized by its own content, a zero minimum is read as
        // min-content and the headline comes out one glyph per line.
        .when(flow, |head| head.min_w_0())
        .child(
            Icon::default()
                .path(tone.icon())
                .size_4()
                .text_color(color)
                .flex_none(),
        )
        .child(
            div()
                .min_w_0()
                .text_color(palette::text_bright())
                .child(headline.into()),
        );
    let reason = move |line: SharedString| {
        div()
            .when(flow, |line| line.min_w_0())
            .text_xs()
            .text_color(palette::text_muted())
            .child(line)
    };
    let shell = div()
        .flex()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_SM)
        // Roomier on the left than the other three sides: the rule and the
        // face are stacked up against that edge, and at even padding they
        // crowd it.
        .pl(tokens::SPACE_MD)
        .rounded(tokens::RADIUS)
        .bg(palette::alpha(color, 0x1c))
        .border_l(px(2.))
        .border_color(color);
    if flow {
        // One wrapping row. Where a line breaks is decided on the items'
        // natural widths, so the reasons ride along beside the headline
        // until they stop fitting and take their own line; min_w_0 is only
        // for the reason too long for even that, which wraps inside itself
        // the way it does stacked.
        return shell
            .flex_row()
            .flex_wrap()
            .items_center()
            .child(head)
            .children(lines.into_iter().map(reason));
    }
    // Detail hangs under the headline's text, clear of the face: the icon
    // plus the gap it sits behind.
    let body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .min_w_0()
        .pl(px(16.) + tokens::SPACE_SM)
        .children(lines.into_iter().map(reason));
    shell.flex_col().child(head).child(body)
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
        crate::settings::ui::icon_button(icon, false, move |_, _, cx| player.update(cx, verb))
    };
    let random = {
        let player = player.clone();
        let library = library.clone();
        crate::settings::ui::icon_button(icons::DICE, false, move |_, _, cx| {
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
    label: impl Into<SharedString>,
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
                .child(label.into())
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

/// One option in a [`mode_list`]: what it's called, what it does, and the
/// value it stands for.
pub struct ModeSpec<V: 'static> {
    pub label: &'static str,
    /// A sentence, not a phrase. The whole reason this control exists rather
    /// than a segmented picker is that these options differ in kind, and a
    /// picker leaves every option but the one you're looking at unexplained.
    pub description: &'static str,
    pub value: V,
}

/// A pick-one list where every option explains itself: a stacked row per
/// option, the chosen one marked and accented.
///
/// For modes that differ in kind rather than degree, where the difference is
/// the thing that needs saying. [`choices`] is still right for a short row of
/// obvious alternatives; this is for the ones that need a sentence each.
///
/// `available` refuses an option the way [`choices_gated`] does: it dims and
/// takes no press, since a mode that can't do anything yet should say so from
/// where it sits rather than vanish and leave nothing to explain.
pub fn mode_list<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [ModeSpec<V>],
    current: V,
    available: impl Fn(V) -> bool,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS);
    for option in options {
        let value = option.value;
        let picked = value == current;
        let usable = available(value);
        let on_pick = on_pick.clone();
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                // No width of its own: the row stretches to the list, which
                // stretches to the page column, and that's what gives the
                // description a width to wrap inside. An explicit `w_full`
                // here is worse than nothing, since a percentage against a
                // parent that hasn't resolved its own width falls back to
                // auto, and the row shrinks to its longest line. `min_w_0` is
                // the CSS one: it stops long copy pushing the row wider than
                // what it was stretched to.
                .min_w_0()
                .p(tokens::SPACE_SM)
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(if picked {
                    palette::accent()
                } else {
                    palette::border()
                })
                .bg(if picked {
                    palette::alpha(palette::accent(), 0x20)
                } else {
                    palette::bg_control()
                })
                .when(!usable, |d| d.opacity(0.5))
                .when(usable && !picked, |d| {
                    d.hover(|d| d.bg(palette::bg_control_hover()))
                        .cursor_pointer()
                })
                .when(usable, |d| {
                    d.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                    )
                })
                // The dot rides the label's own line rather than the whole
                // row, so it centers on the text at any app font size instead
                // of floating against the top of a description that wrapped.
                // It says pick-one where a check would say on-and-off, which
                // is the wrong promise for a list only one row can win.
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(
                            div()
                                .flex_none()
                                .size(px(10.))
                                .rounded_full()
                                .border_1()
                                .border_color(if picked {
                                    palette::accent()
                                } else {
                                    palette::text_faint()
                                })
                                .when(picked, |d| d.bg(palette::accent())),
                        )
                        .child(div().text_color(palette::text()).child(option.label)),
                )
                // Indented past the dot so the description reads as the
                // label's, not as another row.
                .child(
                    div()
                        .pl(px(10.) + tokens::SPACE_SM)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(option.description),
                ),
        );
    }
    list
}

/// The settings-page sliders' strip width and the readout beside them.
pub const SLIDER_W: Pixels = px(150.);
pub const READOUT_W: Pixels = px(60.);

/// How wide a scrub strip draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SliderWidth {
    /// [`SLIDER_W`], so every slider down a settings page lines up in one
    /// control column whatever its label.
    Fixed,
    /// Whatever room the parent gives it. For a dialog, where there's no
    /// column to line up with and a short strip adrift in a wide box reads
    /// as a layout mistake rather than a choice.
    Fill,
}

/// The scrub strip alone: the shared slider chrome over a drag surface,
/// applying the strip fraction live on click and drag. The row builders
/// below pair it with their readout.
fn slider_strip<P: 'static>(
    scrub: &ScrubState,
    fraction: f32,
    width: SliderWidth,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let entity = cx.entity();
    div()
        .map(|d| match width {
            SliderWidth::Fixed => d.w(SLIDER_W).flex_none(),
            SliderWidth::Fill => d.flex_1(),
        })
        .h(tokens::CONTROL_H)
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
        )
}

/// One in-flight readout edit across a panel's settings sliders: which
/// strip is being typed into and the input holding the text. One per
/// panel, behind Arcs like [`ScrubState`], so the row builders only need
/// a read and a second click simply moves the edit.
#[derive(Clone, Default)]
pub struct ValueEdit {
    inner: Arc<Mutex<ValueEditInner>>,
}

#[derive(Default)]
struct ValueEditInner {
    active: Option<usize>,
    input: Option<Entity<InputState>>,
    /// Keeps the enter/blur subscription alive exactly as long as the
    /// edit; replaced wholesale when the edit moves to another strip.
    events: Option<Subscription>,
    /// Where the input painted, for the click-outside cancel: a press
    /// anywhere else abandons the edit without committing.
    bounds: Option<Bounds<Pixels>>,
}

impl ValueEdit {
    /// The input to render for strip `id` while it is the one being
    /// edited.
    pub fn editing(&self, id: usize) -> Option<Entity<InputState>> {
        let inner = self.inner.lock().unwrap();
        if inner.active == Some(id) {
            inner.input.clone()
        } else {
            None
        }
    }

    fn active_id(&self) -> Option<usize> {
        self.inner.lock().unwrap().active
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        self.inner.lock().unwrap().bounds = Some(bounds);
    }

    fn contains(&self, position: Point<Pixels>) -> bool {
        self.inner
            .lock()
            .unwrap()
            .bounds
            .is_some_and(|bounds| bounds.contains(&position))
    }

    fn begin(&self, id: usize, input: Entity<InputState>, events: Subscription) {
        let mut inner = self.inner.lock().unwrap();
        inner.active = Some(id);
        inner.input = Some(input);
        inner.events = Some(events);
        inner.bounds = None;
    }

    fn end(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.active = None;
        inner.input = None;
        inner.events = None;
        inner.bounds = None;
    }
}

/// [`value_slider`] whose readout doubles as an input: click the number,
/// type, Enter commits, blur cancels. `edit_text` seeds the field with the
/// bare number, no unit; `to_fraction` maps the typed value back into the
/// strip's 0..1 through the row's own mapping (linear, log, whatever the
/// slider itself runs), and the result clamps to the strip before it
/// applies.
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    value_slider_edit_over(
        scrub,
        edit,
        fraction,
        readout,
        edit_text,
        1.0,
        to_fraction,
        apply,
        cx,
    )
}

/// [`value_slider_edit`] with typed headroom past the strip's top: `over`
/// is the highest fraction a typed value may reach, for knobs whose
/// slider range is a sensible reach rather than a law. The strip still
/// scrubs its own span and pins full while the value sits beyond it.
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_over<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    value_slider_edit_sized(
        scrub,
        edit,
        fraction,
        readout,
        edit_text,
        over,
        SliderWidth::Fixed,
        to_fraction,
        apply,
        cx,
    )
}

/// The same, with the strip's width said out loud. The settings pages want
/// [`SliderWidth::Fixed`] and get it from the wrapper above; a dialog builds
/// its own row and asks for [`SliderWidth::Fill`].
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_sized<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    width: SliderWidth,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        // A filling strip only fills if the row it sits in does too.
        .map(|d| match width {
            SliderWidth::Fixed => d,
            SliderWidth::Fill => d.w_full(),
        })
        .child(slider_strip(scrub, fraction, width, apply.clone(), cx));
    if let Some(input) = edit.editing(scrub.id()) {
        // While the edit is live, a one-frame window handler (the
        // scrub_on_paint idiom) watches for a press outside the input and
        // abandons the edit uncommitted: nothing else in the settings
        // window takes focus, so blur alone never fires.
        let id = scrub.id();
        let entity = cx.entity();
        return row.child(
            div()
                .w(READOUT_W)
                // Pinned to the strip's height with the input centered: the
                // small input is 2px taller than CONTROL_H (its border), and
                // left to size the row it nudges the whole page on toggle.
                .h(tokens::CONTROL_H)
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .child(
                    canvas(
                        {
                            let edit = edit.clone();
                            move |bounds, _, _| edit.set_bounds(bounds)
                        },
                        {
                            let edit = edit.clone();
                            move |_, _, window, _| {
                                let edit = edit.clone();
                                let entity = entity.clone();
                                window.on_mouse_event(
                                    move |event: &MouseDownEvent, phase, _, cx| {
                                        if !phase.bubble()
                                            || edit.active_id() != Some(id)
                                            || edit.contains(event.position)
                                        {
                                            return;
                                        }
                                        edit.end();
                                        entity.update(cx, |_, cx| cx.notify());
                                    },
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(Input::new(&input).small().w_full()),
        );
    }
    let id = scrub.id();
    row.child(
        div()
            .w(READOUT_W)
            .flex_none()
            .text_right()
            .text_color(palette::text_muted())
            // The hover cue is a background, never a text restyle: a hover
            // text refinement re-shapes the line with its own metrics and
            // the number visibly shifts under the pointer.
            .rounded(tokens::RADIUS)
            .hover(|d| d.bg(palette::bg_control()))
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let edit = edit.clone();
                    move |_: &mut P, _, window, cx| {
                        let input = cx
                            .new(|cx| InputState::new(window, cx).default_value(edit_text.clone()));
                        let events = cx.subscribe_in(&input, window, {
                            let edit = edit.clone();
                            let to_fraction = to_fraction.clone();
                            let apply = apply.clone();
                            move |this: &mut P, input, event: &InputEvent, _, cx| match event {
                                InputEvent::PressEnter { .. } => {
                                    let text = input.read(cx).value().trim().replace(',', ".");
                                    if let Ok(value) = text.parse::<f32>() {
                                        let ceiling = over.max(1.0);
                                        apply(this, to_fraction(value).clamp(0.0, ceiling), cx);
                                    }
                                    edit.end();
                                    cx.notify();
                                }
                                InputEvent::Blur => {
                                    edit.end();
                                    cx.notify();
                                }
                                _ => {}
                            }
                        });
                        window.focus(&input.read(cx).focus_handle(cx));
                        edit.begin(id, input, events);
                        cx.notify();
                    }
                }),
            )
            .child(readout),
    )
}

/// The switch pill and knob without any interaction, shared by [`toggle`],
/// [`toggle_locked`], and the menu rows that flip a switch from their own
/// click rather than from the widget.
pub fn toggle_face(on: bool) -> Div {
    toggle_track(on)
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

/// A dropdown over a list of choices: a small button labeled with whichever
/// option is current, its menu the whole list with a tick on that one. Use
/// it where [`choices`] would run out of room, a picker whose list is
/// however many the machine happens to have rather than a fixed two or
/// three. `disabled` draws it inert, for a knob whose mode doesn't apply.
pub fn picker<P, K>(
    id: &'static str,
    current: K,
    options: Vec<(K, SharedString)>,
    disabled: bool,
    apply: impl Fn(&mut P, K, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement
where
    P: 'static,
    K: PartialEq + Clone + 'static,
{
    // An id that isn't in the list still has to label the button, so fall
    // back to the head rather than drawing an empty one: a device that was
    // unplugged since the pick reads as the default it will actually open.
    // The tick follows the same fallback, so the open menu points at the row
    // the button is already naming instead of checking nothing.
    let picked = options
        .iter()
        .find(|(key, _)| *key == current)
        .or_else(|| options.first());
    let label = picked.map(|(_, label)| label.clone()).unwrap_or_default();
    let current = picked.map(|(key, _)| key.clone());
    let weak = cx.entity().downgrade();
    Button::new(id)
        .label(label)
        .small()
        .outline()
        .disabled(disabled)
        .dropdown_menu(move |mut menu, _, _| {
            for (key, label) in options.iter() {
                let checked = current.as_ref() == Some(key);
                let key = key.clone();
                let pick = weak.clone();
                let apply = apply.clone();
                menu = menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        let key = key.clone();
                        let apply = apply.clone();
                        if let Some(this) = pick.upgrade() {
                            this.update(cx, |this, cx| apply(this, key, cx));
                        }
                    },
                ));
            }
            menu
        })
}

/// A font-family picker: the shared dropdown over the installed families,
/// with a Default at the head that clears the override back to the app
/// font. `current` is the panel's stored family, None meaning inherit.
pub fn font_picker<P: 'static>(
    id: &'static str,
    current: Option<String>,
    apply: impl Fn(&mut P, Option<String>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
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
    let mut options: Vec<(Option<String>, SharedString)> = vec![(None, "Default".into())];
    options.extend(
        fonts
            .iter()
            .map(|name| (Some(name.to_string()), name.clone())),
    );
    picker(id, current, options, false, apply, cx)
}

/// The chrome shared by the segmented pickers and the toggle groups: a
/// joined group of segments, the picked ones filled with the accent,
/// hairline gaps between the rest. The predicate says which segments
/// read as on; the exclusive pickers pass equality with the current
/// value, the toggle groups each flag's own state.
fn segments<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    picked: impl Fn(V) -> bool,
    available: impl Fn(V) -> bool,
    render: impl Fn(&'static str, bool) -> AnyElement,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let last = options.len().saturating_sub(1);
    let mut group = div().flex().flex_row().flex_none().items_center();
    for (i, (key, value)) in options.iter().enumerate() {
        let value = *value;
        let picked = picked(value);
        // A segment nothing can pick is dimmed and inert, `toggle_locked`'s
        // treatment: it keeps its place in the group so the choice still
        // reads as a choice, and says without a click that it isn't one
        // right now.
        let available = available(value);
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
                .when(!available, |d| d.opacity(0.5))
                .when(available, |d| {
                    d.when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                        )
                })
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
    choices_gated(options, current, |_| true, on_pick, cx)
}

/// [`choices`] where some options can't be taken yet: whatever `available`
/// refuses is dimmed and swallows no press.
///
/// For a choice that exists but needs something first, where dropping the
/// option entirely would leave the row unable to say what's missing. The
/// description beside it is what explains why; this only stops the press
/// that would otherwise land and appear to do nothing.
pub fn choices_gated<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    available: impl Fn(V) -> bool,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        move |value| value == current,
        available,
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
        move |value| value == current,
        |_| true,
        icon_segment,
        on_pick,
        cx,
    )
}

/// A joined group of independent icon toggles: the segmented pickers'
/// chrome, but each segment flips its own flag instead of one pick
/// excluding the rest.
pub fn icon_toggles<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    active: impl Fn(V) -> bool,
    on_toggle: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(options, active, |_| true, icon_segment, on_toggle, cx)
}

/// One icon segment's face, shared by the exclusive picker and the
/// toggle group.
fn icon_segment(icon: &'static str, picked: bool) -> AnyElement {
    svg()
        .path(icon)
        .size_4()
        .text_color(if picked {
            palette::text_on_accent()
        } else {
            palette::text()
        })
        .into_any_element()
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

/// Where a panel's content sits vertically, the companion to [`Align`]
/// for a panel that has height to spare.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Apply a vertical alignment along a column's main axis.
pub fn justify_v(d: Div, align: VAlign) -> Div {
    match align {
        VAlign::Top => d.justify_start(),
        VAlign::Middle => d.justify_center(),
        VAlign::Bottom => d.justify_end(),
    }
}

/// The vertical alignment setting row, the companion to [`align_row`].
pub fn valign_row<P: 'static>(
    current: VAlign,
    on_pick: impl Fn(&mut P, VAlign, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        "Vertical Alignment",
        Some("Where the content sits when the panel has height to spare"),
        choices(
            &[
                ("Top", VAlign::Top),
                ("Middle", VAlign::Middle),
                ("Bottom", VAlign::Bottom),
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
        let state = self.state.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            dock_back_item(menu, panel, state)
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
                // A panel that serves its own content menu already carries
                // Dock Back in that menu's Panel tail, so the window's own
                // right-click would only stack a second menu on top. Install
                // it only as the fallback for panels with no content menu.
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
            path: Some("/tmp/smudge.wgsl".into()),
            routes: vec![rox_viz::signal::Route {
                enabled: true,
                signal: 3,
                target: shader::slot_target(1),
                from: 0.0,
                to: 1.0,
            }],
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
}
