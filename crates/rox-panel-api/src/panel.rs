//! The app's own panel layer per ADR 7. The dock, tabs, splits and resize
//! come from gpui-component. Panels are views over the shared entities in
//! [`AppState`], so a duplicate is a second view with its own config over
//! the same state, and a popped-out panel is the same entity rehosted in
//! its own OS window.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    AbsoluteLength, AnyElement, App, Bounds, ClipboardItem, Context, DismissEvent, Div, Element,
    Entity, FocusHandle, Focusable as _, GlobalElementId, HighlightStyle, InspectorElementId,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Rgba, SharedString, Size,
    Stateful, StyledText, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowHandle, WindowOptions, anchored, deferred, div, fill, linear_color_stop, linear_gradient,
    point, prelude::*, px, relative, size,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Root};
use rox_dock::{OpenPanelSettings, Panel, PanelInfo, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::actions::{SeekBackward, SeekForward, TogglePlayback};
use crate::query::shared_query::SharedQuery;
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette::PanelTheme;
use rox_design::{palette, tokens};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::cues::Cues;
use rox_services::discord_presence::DiscordPresence;
use rox_services::history::History;
use rox_services::lastfm::Scrobbler;
use rox_services::librefm::LibreFm;
use rox_services::listenbrainz::ListenBrainz;
use rox_services::player::{AbState, FadeView, Player, fmt_time};
use rox_services::portraits::Portraits;
use rox_services::radio::Radio;
use rox_services::selection::Selection;
use rox_services::thumbs::Thumbs;

pub mod arrange;
pub use arrange::*;

pub mod shader;
pub use shader::PanelShader;

// Panels reach the rox-panel-kit widget layer through crate::panel.
pub use rox_panel_kit::{
    Align, FlickState, ModeSpec, PANEL_NAV_CONTEXT, PatternNote, ResumeIdle, SLIDER_STEP,
    ScrubState, SliderWidth, TYPE_AHEAD_CYCLE_CONTEXT, Tip, Tone, TrackedImage, VAlign, ValueEdit,
    align_row, banner, banner_flow, check_row, choices, choices_gated, choices_icons,
    choices_shared, display_name, flick_on_paint_axis, follow_panel, font_picker, glide_snap_axis,
    glide_step, glide_step_axis, glide_target, glide_target_at, glide_target_axis, icon_choices,
    icon_control, icon_control_sized, icon_toggles, items, justify, justify_v, language_picker,
    letter_initial, letter_rail, mode_list, paint_slider, panel_nav_context, pattern_input, picker,
    scrub_on_paint, setting_block, setting_row, setting_row_dyn, title_text, toggle, toggle_face,
    toggle_locked, tracking_section, type_ahead_context, type_ahead_fade, type_ahead_grow,
    type_ahead_hit, type_ahead_live, type_ahead_overlay, type_ahead_scan, valign_row,
    value_slider_edit, value_slider_edit_over, value_slider_edit_sized, window_body,
    workspace_body,
};

/// The shared entities every panel renders over. Cloning shares the
/// handles, not the state.
#[derive(Clone)]
pub struct AppState {
    pub library: Entity<Library>,
    pub player: Entity<Player>,
    pub selection: Entity<Selection>,
    /// This run's cue points, never written down. One per workspace so a strip,
    /// its duplicate and the key commands all mark the same song.
    pub cues: Entity<Cues>,
    pub query: Entity<SharedQuery>,
    pub tab_hosts: Entity<TabHosts>,
    /// One bake shared by every window over this player.
    pub now_art: Entity<NowPlayingArt>,
    pub thumbs: Entity<Thumbs>,
    pub portraits: Entity<Portraits>,
    /// One per player: a second copy would fire the scrobble and the cover
    /// lookup twice per song.
    pub radio: Entity<Radio>,
    /// Also holds the live scrobble config, for the panels' threshold markers.
    pub scrobbler: Entity<Scrobbler>,
    /// Also holds its live config, for the settings page.
    pub listenbrainz: Entity<ListenBrainz>,
    pub librefm: Entity<LibreFm>,
    pub history: Entity<History>,
    pub discord: Entity<DiscordPresence>,
    /// The shared modulation pool. Panels tick it from their paint and read
    /// values; edits persist through settings.
    pub signals: Arc<rox_viz::signal::SignalHub>,
}

impl AppState {
    /// Where the scrobble threshold line goes, 0 to 1, or None while no
    /// destination would send on it. It depends on the switch and all three
    /// accounts, which is why it's asked here.
    pub fn scrobble_marker(&self, cx: &gpui::App) -> Option<f32> {
        let scrobbler = self.scrobbler.read(cx);
        let armed = scrobbler.scrobbling()
            && (scrobbler.connected()
                || self.listenbrainz.read(cx).connected()
                || self.librefm.read(cx).connected());
        armed.then(|| scrobbler.marker()).flatten()
    }
}

/// Every tab panel that has hosted one of our panels, reported from
/// `on_added_to`. The dock creates tab panels on its own when a tab is
/// dragged into a split and announces none of them, so this is how the
/// Panels menu finds a live one.
#[derive(Default)]
pub struct TabHosts {
    hosts: Vec<WeakEntity<TabPanel>>,
}

impl TabHosts {
    pub fn report(&mut self, tabs: WeakEntity<TabPanel>) {
        if self.hosts.iter().any(|t| t.entity_id() == tabs.entity_id()) {
            return;
        }
        self.hosts.push(tabs);
    }

    pub fn last_live(&self, cx: &App) -> Option<Entity<TabPanel>> {
        self.hosts.iter().rev().find_map(|tabs| {
            let tabs = tabs.upgrade()?;
            tabs.read(cx).visible(cx).then_some(tabs)
        })
    }
}

/// Focus the first open panel with this built-in name across every tab
/// group. Popped-out panels live in their own windows, so they never match.
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

/// [`icon_control`] with a crossfade running through it: an accent wash
/// sweeps across in the skip's direction, its soft edge at the fade's
/// progress. None is the plain button.
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
            // Soft-edged, since a moving hard line reads as a progress bar.
            let wash = palette::alpha(palette::accent(), 0x66);
            let clear = palette::alpha(palette::accent(), 0x00);
            let at = fade.progress();
            d.bg(linear_gradient(
                // 270 runs right to left, so a Previous sweeps back.
                if fade.back { 270. } else { 90. },
                linear_color_stop(wash, (at - EDGE).max(0.0)),
                linear_color_stop(clear, (at + EDGE).min(1.0)),
            ))
        })
        // A completed fade leaves the button washed, and cutting that to nothing
        // reads as a glitch, so it flashes one notch brighter and dissolves.
        .when_some(outro, |d, strength| {
            d.bg(palette::alpha(
                palette::accent(),
                (0x99 as f32 * strength) as u8,
            ))
        })
}

/// The sweep edge's blur either side of the fade position, as a fraction of the button.
const EDGE: f32 = 0.2;

/// A's fraction of the track, and B's once marked. None while nothing is
/// marked or the duration hasn't resolved.
pub fn ab_fractions(ab: AbState, duration_secs: Option<f64>) -> Option<(f32, Option<f32>)> {
    let duration = duration_secs.filter(|d| *d > 0.0)?;
    let frac = |secs: f64| (secs / duration) as f32;
    match ab {
        AbState::Off => None,
        AbState::ASet(a) => Some((frac(a), None)),
        AbState::Looping(a, b) => Some((frac(a), Some(frac(b)))),
    }
}

/// Paint the A-B section over a strip: a line at each end and an accent
/// wash between them. With only A down the line stands alone. `weight`
/// scales every alpha, for a strip fading its shape in or out.
pub fn paint_ab(
    ab: Option<(f32, Option<f32>)>,
    weight: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let Some((a, b)) = ab else {
        return;
    };
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let level = |max: u8| (max as f32 * weight.clamp(0.0, 1.0)) as u8;
    let a_x = a.clamp(0.0, 1.0) * w;
    if let Some(b) = b {
        let b_x = b.clamp(0.0, 1.0) * w;
        let wash = level(AB_WASH);
        if wash > 0 && b_x > a_x {
            window.paint_quad(fill(
                Bounds::new(
                    point(bounds.origin.x + px(a_x), bounds.origin.y),
                    size(px(b_x - a_x), px(h)),
                ),
                palette::alpha(palette::accent(), wash),
            ));
        }
    }
    let line = level(AB_LINE);
    if line == 0 {
        return;
    }
    for x in [Some(a_x), b.map(|b| b.clamp(0.0, 1.0) * w)]
        .into_iter()
        .flatten()
    {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x - AB_LINE_W / 2.0), bounds.origin.y),
                size(px(AB_LINE_W), px(h)),
            ),
            palette::alpha(palette::accent(), line),
        ));
    }
}

/// Light enough that the played side's own fill reads through.
const AB_WASH: u8 = 0x30;
const AB_LINE: u8 = 0xcc;
/// A touch under the playhead, so the head stands out when it crosses one.
const AB_LINE_W: f32 = 1.5;

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

/// The time under the pointer as a pill that follows the cursor. Drop it
/// over the strip's relative container: it covers the strip to catch every
/// move, and a click bubbles through to the strip's own seek handler.
pub fn seek_hover<V: 'static>(
    scrub: &ScrubState,
    duration: f64,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let moved = scrub.clone();
    let left = scrub.clone();
    let hover = scrub.hover();
    div()
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
            if !hovered && left.set_hover(None) {
                cx.notify();
            }
        }))
        .when_some(hover, |d, fraction| d.child(seek_pill(fraction, duration)))
}

/// A zero-width column at the fraction centers the pill on the cursor.
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
                // The zero-width column gives the text no room, so a time would wrap per glyph.
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

/// Repaint the tab panel hosting a renamed panel. The tab bar only repaints
/// when the tab panel itself is notified.
pub fn refresh_tab_panel(tab_panel: &Option<WeakEntity<TabPanel>>, cx: &mut App) {
    if let Some(tabs) = tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |_, cx| cx.notify());
    }
}

pub fn config_from_info<C: Default + serde::de::DeserializeOwned>(info: &PanelInfo) -> C {
    match info {
        PanelInfo::Panel(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        _ => C::default(),
    }
}

/// The Dock Back entry: moves a popped-out panel into the newest live tab
/// group and closes its window. Cross-window drags can't bring a panel home
/// (a held button pins pointer events to its window, and Wayland hides
/// window positions), so this menu is the way back.
pub fn dock_back_item(menu: PopupMenu, panel: Arc<dyn PanelView>, state: AppState) -> PopupMenu {
    let hosts = state.tab_hosts.clone();
    menu.item(
        PopupMenuItem::new(rox_i18n::t!("panel-dock-back"))
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

/// The Pop Out and Close tail of a panel's menu. Pass the tab panel it's
/// in, from `on_added_to`. Close lives here so every panel has it wherever
/// its menu shows; for a solo content panel it's the only close there is.
/// Popped out there's no Close, since closing the window is the close.
pub fn popout_item<P: Panel>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &Window,
) -> PopupMenu {
    // No tab strip: either popped out, where the way home belongs, or in a
    // composite's slot, where the tail ends.
    let Some(tabs) = tab_panel.clone() else {
        if !dock_back_offered(window) {
            return menu;
        }
        // Kept out of design mode, unlike the rows below: a popped-out window has
        // no other way back into the layout.
        return dock_back_item(menu, Arc::new(panel.clone()), state);
    };
    if !settings::design_mode() {
        return menu;
    }
    let pop_panel = panel.clone();
    let pop_tabs = tab_panel;
    let menu = menu.item(
        PopupMenuItem::new(rox_i18n::t!("panel-pop-out"))
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
        PopupMenuItem::new(rox_i18n::t!("panel-close"))
            .icon(Icon::default().path(icons::CLOSE))
            .on_click(move |_, window, cx| {
                if panel.read(cx).locked(cx) {
                    // The pin absorbs a stray click, so route it to a confirm. With no
                    // workspace behind the window, the pin just holds.
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

/// The Duplicate entry: a second panel of the same type in this one's tab
/// strip, config copied. `make` builds the copy from the source, since each
/// panel's `new` takes a different shape. No tab strip, no entry.
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
        PopupMenuItem::new(rox_i18n::t!("panel-duplicate"))
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

/// Reveal a track's file in the platform file manager. The id resolves to
/// its path at click time, so a re-scanned file is still found.
pub fn reveal_item(menu: PopupMenu, state: AppState, id: Option<i64>) -> PopupMenu {
    let Some(id) = id else {
        return menu;
    };
    menu.item(
        PopupMenuItem::new(rox_i18n::t!("panel-reveal-in-browser"))
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

/// What the Copy submenu can put on the clipboard for one track. Resolved
/// at click time, so a copy after a rescan reads the file where it is now.
pub struct CopyText {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl CopyText {
    /// A file the library doesn't know still copies its path.
    pub fn from_key(key: &rox_library::cue::TrackKey, library: &Library) -> Self {
        CopyText::from_tags(key, library.meta_for_key(key).as_ref())
    }

    /// The playing track's fields, with a station's current song laid over the
    /// row so a copy gets the song rather than the station. None while idle.
    pub fn playing(state: &AppState, cx: &App) -> Option<Self> {
        let player = state.player.read(cx);
        let key = player.now_playing()?.key;
        let meta = player.now_meta(state.library.read(cx));

        Some(CopyText::from_tags(&key, meta.as_ref()))
    }

    fn from_tags(
        key: &rox_library::cue::TrackKey,
        meta: Option<&rox_library::store::TrackMeta>,
    ) -> Self {
        CopyText {
            path: key.path.clone(),
            title: meta.map(|m| m.title.clone()).unwrap_or_default(),
            artist: meta.map(|m| m.artist.clone()).unwrap_or_default(),
            album: meta.map(|m| m.album.clone()).unwrap_or_default(),
        }
    }
}

/// Names the tracks a Copy entry acts on, run at click time.
pub type CopyResolver = Rc<dyn Fn(&App) -> Vec<CopyText>>;

/// The Copy submenu shared by every track surface. `resolve` runs on click.
/// A multi-row selection copies one line per track with empty fields
/// dropped, and nothing is written when no track has the field. Text only:
/// gpui's clipboard carries strings and images, so handing a file manager
/// the file itself would take a platform patch.
pub fn copy_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut App,
    resolve: CopyResolver,
) -> PopupMenu {
    type Pick = fn(&CopyText) -> String;
    let entries: [(SharedString, &str, Pick); 5] = [
        (rox_i18n::t!("panel-copy-title"), icons::FILE_TEXT, |t| {
            t.title.clone()
        }),
        (rox_i18n::t!("panel-copy-artist"), icons::MIC, |t| {
            t.artist.clone()
        }),
        (rox_i18n::t!("panel-copy-album"), icons::DISC, |t| {
            t.album.clone()
        }),
        (rox_i18n::t!("panel-copy-filename"), icons::FILE_TEXT, |t| {
            t.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        }),
        (rox_i18n::t!("panel-copy-path"), icons::FOLDER, |t| {
            t.path.to_string_lossy().into_owned()
        }),
    ];
    let submenu = PopupMenu::build(window, cx, move |mut submenu, _, _| {
        for (label, icon, pick) in entries {
            let resolve = resolve.clone();
            submenu = submenu.item(
                PopupMenuItem::new(label)
                    .icon(Icon::default().path(icon))
                    .on_click(move |_, _, cx| {
                        let lines: Vec<String> = resolve(cx)
                            .iter()
                            .map(pick)
                            .filter(|line| !line.is_empty())
                            .collect();
                        if !lines.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
                        }
                    }),
            );
        }
        submenu
    });
    menu.item(
        PopupMenuItem::submenu(rox_i18n::t!("panel-copy"), submenu)
            .icon(Icon::default().path(icons::COPY)),
    )
}

/// [`copy_submenu`] over library ids, resolved at click time. Ids the
/// library has since dropped fall out.
pub fn copy_ids_submenu(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    if ids.is_empty() {
        return menu;
    }
    copy_submenu(
        menu,
        window,
        cx,
        Rc::new(move |cx: &App| {
            let library = state.library.read(cx);
            library
                .keys_for(&ids)
                .unwrap_or_default()
                .iter()
                .map(|key| CopyText::from_key(key, library))
                .collect()
        }),
    )
}

/// Queue tracks after the playing one when `next`, at the tail otherwise.
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

/// Add to Playlist: Create New, then every static list, built at open time.
/// Split out of [`track_actions`] because a station can join a list but has
/// nothing to tag, rename or convert.
pub fn playlist_item(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let submenu = PopupMenu::build(window, cx, move |mut submenu, _window, cx| {
        let new_state = state.clone();
        let new_ids = ids.clone();
        submenu = submenu.item(
            PopupMenuItem::new(rox_i18n::t!("panel-new-playlist"))
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    crate::openers::playlist_create(new_state.clone(), new_ids.clone(), cx);
                }),
        );
        // Static lists only: a smart playlist holds what its query returns.
        let playlists: Vec<_> = state
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
            let add_state = state.clone();
            let add_ids = ids.clone();
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

    menu.item(
        PopupMenuItem::submenu(rox_i18n::t!("panel-add-to-playlist"), submenu)
            .icon(Icon::default().path(icons::LIST_MUSIC)),
    )
}

/// The track actions every song surface's right-click shares. Play differs
/// per panel, so the caller hands the click over. The ids resolve at build
/// time, so the editors get this set even if another panel publishes over
/// the shared selection first. File actions (the editors, rename, convert,
/// reveal) take only the ids that are files and hide when there are none,
/// so a server's song or a station never reaches an editor.
pub fn track_actions(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    play_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
    on_play: impl Fn(&mut Window, &mut App) + 'static,
) -> PopupMenu {
    let files = state.library.read(cx).local_ids(&ids);
    let reveal = files.first().copied();
    let mark_ids = ids.clone();
    let mark_state = state.clone();
    let tag_ids = files.clone();
    let tag_state = state.clone();
    let cover_state = state.clone();
    let cover_ids = files.clone();
    let rename_ids = files.clone();
    let rename_state = state.clone();
    let convert_ids = files.clone();
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
        .item(
            PopupMenuItem::new(rox_i18n::t!("panel-play-next"))
                .icon(Icon::default().path(icons::SKIP_FORWARD))
                .on_click(move |_, _, cx| {
                    queue_tracks(&next_state, &next_ids, true, cx);
                }),
        )
        .item(
            PopupMenuItem::new(rox_i18n::t!("panel-add-to-queue"))
                .icon(Icon::default().path(icons::LIST_MUSIC))
                .on_click(move |_, _, cx| {
                    queue_tracks(&queue_state, &queue_ids, false, cx);
                }),
        );
    let favourites = state.library.read(cx).favourite_ids();
    let all_fav = !ids.is_empty() && ids.iter().all(|id| favourites.contains(id));
    let fav_state = state.clone();
    let fav_ids = ids.clone();
    let (fav_label, fav_icon) = if all_fav {
        (rox_i18n::t!("panel-favourite-remove"), icons::HEART_FILLED)
    } else {
        (rox_i18n::t!("panel-favourite-add"), icons::HEART)
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
    let menu = playlist_item(menu, playlist_state, playlist_ids, window, cx);
    let marks = mark_state.library.read(cx).bookmark_count_for(&mark_ids);
    let menu = menu.when(marks > 0, |menu| {
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("panel-remove-bookmarks", count = marks))
                .icon(Icon::default().path(icons::BOOKMARK))
                .on_click(move |_, _, cx| {
                    let ids = mark_ids.clone();
                    mark_state
                        .library
                        .update(cx, |library, cx| library.remove_track_bookmarks(&ids, cx));
                }),
        )
    });
    let has_files = !files.is_empty();
    let menu = menu.when(has_files, |menu| {
        menu
            // The tag editor is the primary editing flow; the metadata
            // panel's inline pencil stays the quick path.
            .item(
                PopupMenuItem::new(rox_i18n::t!("panel-edit-tags"))
                    .icon(Icon::default().path(icons::PENCIL))
                    .on_click(move |_, _, cx| {
                        crate::openers::tags_editor(tag_state.clone(), tag_ids.clone(), cx);
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("panel-edit-cover"))
                    .icon(Icon::default().path(icons::IMAGE))
                    .on_click(move |_, _, cx| {
                        crate::openers::cover_editor(cover_state.clone(), cover_ids.clone(), cx);
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("panel-rename-files"))
                    .icon(Icon::default().path(icons::FOLDER))
                    .on_click(move |_, _, cx| {
                        crate::openers::rename_dialog(rename_state.clone(), rename_ids.clone(), cx);
                    }),
            )
    });
    let menu = if has_files && crate::openers::convert_available() {
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("panel-convert"))
                .icon(Icon::default().path(icons::AUDIO_LINES))
                .on_click(move |_, _, cx| {
                    crate::openers::convert_dialog(convert_state.clone(), convert_ids.clone(), cx);
                }),
        )
    } else {
        menu
    };
    let menu = copy_ids_submenu(menu, state.clone(), ids, window, cx);
    reveal_item(menu, state, reveal)
}

/// Move a docked panel into its own OS window. The entity itself moves, so
/// it keeps rendering the same shared state.
pub fn pop_out<P: Panel>(
    panel: Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    state: AppState,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(tabs) = tab_panel.and_then(|tabs| tabs.upgrade()) {
        tabs.update(cx, |tabs, cx| {
            tabs.remove_panel(Arc::new(panel.clone()), window, cx);
        });
    }
    pop_out_view(Arc::new(panel), state, cx);
}

/// Open an OS window hosting a detached panel. Also the dock's drag-out hook.
pub fn pop_out_view(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    panel_window(panel, state, false, cx);
}

/// Open a panel straight into a window of its own (New Window from Panel).
/// It never came out of a dock, so it gets no way back.
pub fn open_panel_window(panel: Arc<dyn PanelView>, state: AppState, cx: &mut App) {
    panel_window(panel, state, true, cx);
}

/// Single-panel windows: true for a pop-out, false for one opened straight
/// into a window. A static rather than a gpui global because menu builders
/// read it with a window and no `App`.
static PANEL_WINDOWS: RwLock<BTreeMap<u64, bool>> = RwLock::new(BTreeMap::new());

/// Only a window a panel popped out into offers Dock Back. Anywhere else, a
/// panel with no tab strip is a composite's child, and Dock Back would close
/// the window out from under its siblings.
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
        window_decorations: Some(rox_core::settings::child_window_decorations()),
        titlebar: Some(child_titlebar(title.clone())),
        app_id: Some(rox_core::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        // Wayland ignores the creation-time title; only set_window_title gets through.
        crate::windows::set_window_title(window, &title);
        settle_child_chrome(window, cx);
        // A popped-out panel keeps its surface shader, so this window needs the hub
        // and player its slots read from.
        shader::note_window(window, &state, cx);
        let window_id = window.window_handle().window_id().as_u64();
        PANEL_WINDOWS.write().unwrap().insert(window_id, !fresh);
        let host = cx.new(|cx| {
            // A popped-out window pumps its own frames, so the backdrop needs its own wake.
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
        host.read(cx).focus.clone().focus(window);
        let framed = crate::fallback_chrome::wrap(host, cx);
        cx.new(|cx| Root::new(framed, window, cx))
    })
    .expect("failed to open the panel window");
}

/// Open a child window hosting `build`'s view in a Root, with the app id so
/// the compositor groups it with the main window. `min_size` floors an
/// interactive resize. The caller keeps its own singleton bookkeeping.
pub fn open_child_window<V: 'static + Render>(
    cx: &mut App,
    title: impl Into<SharedString>,
    bounds: Bounds<Pixels>,
    min_size: Option<Size<Pixels>>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
) -> WindowHandle<Root> {
    open_window(cx, title, bounds, min_size, true, build)
}

/// [`open_child_window`] the user can't resize, for a dialog with one set layout.
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
        window_decorations: Some(rox_core::settings::child_window_decorations()),
        titlebar: Some(child_titlebar(title.clone())),
        app_id: Some(rox_core::APP_ID.into()),
        ..Default::default()
    };
    cx.open_window(options, move |window, cx| {
        crate::windows::set_window_title(window, &title);
        settle_child_chrome(window, cx);
        // `WindowOptions::focus` is already true, but some window managers grant
        // the map and deny the raise, so keys keep going to the old window until a
        // click. Ask explicitly.
        window.activate_window();
        let view = build(window, cx);
        let framed = crate::fallback_chrome::wrap(view, cx);
        cx.new(|cx| Root::new(framed, window, cx))
    })
    .expect("failed to open child window")
}

/// On Windows and macOS the caption follows `appears_transparent`, not
/// `window_decorations`, so a bare child window asks here too and never
/// flashes the OS frame.
fn child_titlebar(title: SharedString) -> TitlebarOptions {
    let bare = matches!(
        rox_core::settings::child_window_decorations(),
        gpui::WindowDecorations::Client
    );

    TitlebarOptions {
        title: Some(title),
        appears_transparent: cfg!(any(target_os = "windows", target_os = "macos")) && bare,
        ..Default::default()
    }
}

/// The frame settings a child window can only take once it's open.
fn settle_child_chrome(window: &mut Window, cx: &mut App) {
    // No WindowOptions field for this one. It only bites on a bare window.
    window.set_resize_border(rox_core::settings::resize_border());

    // macOS still shows native traffic lights under a transparent titlebar, so
    // a bare window asks again to hide them. Deferred: the style mask change
    // can fire a resize back into the window this closure is building.
    let mode = rox_core::settings::child_window_decorations();
    if cfg!(target_os = "macos") && matches!(mode, gpui::WindowDecorations::Client) {
        let handle = window.window_handle();
        cx.defer(move |cx| {
            handle
                .update(cx, |_, window, _| window.request_decorations(mode))
                .ok();
        });
    }
}

/// The frame-level config every panel stores, flattened into its own config
/// with `#[serde(flatten)]`: the knobs that mean the same thing on any
/// panel. Panel-specific fields stay on the panel's config.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct PanelChrome {
    /// The tab and title rename; None shows the built-in name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
    /// Pin the panel so the dock won't drag or rearrange it. Resizing is
    /// handled at the split level.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Make the body a window-move handle, so a decorations-off layout can be
    /// moved by a toolbar strip. Meant for quiet panels.
    #[serde(default, skip_serializing_if = "is_false")]
    pub anchor: bool,
    /// Drop the controls a panel floats over its content (a composition host's
    /// slot buttons, the metadata panel's edit toolbar) for a finished look.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hide_controls: bool,
    /// Width cap in px. A growing window hands the extra room to neighbors, so
    /// a toolbar pinned narrow stays narrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    /// Height cap in px, which keeps a menu bar or footer from stretching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    /// Width floor in px, taken as written even under the panel's built-in floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    /// The vertical twin of [`min_width`](Self::min_width).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
    /// A WGSL shader over the panel's surface, run after its body paints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<PanelShader>,
}

impl PanelChrome {
    /// Its own [`hide_controls`](Self::hide_controls), or design mode off.
    /// Panels call this rather than reading the field; the metadata panel's
    /// edit toolbar is the deliberate exception.
    pub fn controls_hidden(&self) -> bool {
        self.hide_controls || !settings::design_mode()
    }
}

/// Every panel's `Panel::max_size`: the chrome's caps, floored at
/// [`chrome_min_size`] because a settings file can ask for min > max and
/// the dock can't do anything sane with that. An unset axis is unbounded.
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

/// Every panel's `Panel::min_size`: the chrome's floors over `floor`, the
/// panel's built-in minimum. A set axis is taken as written, below the
/// built-in floor included, so a compact layout can go smaller. Zero is
/// the bottom.
pub fn chrome_min_size(chrome: &PanelChrome, floor: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    let axis = |min: Option<f32>, floor: Pixels| match min {
        Some(px_value) => px(px_value.max(0.)),
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

/// What the settings sidebar shows for a [`PanelSettings::pages`] name. The
/// names are a small closed set, so they resolve here rather than at each
/// declaration. An unknown name shows as written.
pub fn page_label(name: &str) -> SharedString {
    let key = match name {
        "Layout" => "panel-page-layout",
        "View" => "panel-page-view",
        "Content" => "panel-page-content",
        "Source" => "panel-page-source",
        "Bindings" => "panel-page-bindings",
        "Emitters" => "panel-page-emitters",
        "Forces" => "panel-page-forces",
        "Scene" => "panel-page-scene",
        other => return SharedString::from(other.to_owned()),
    };
    rox_i18n::t!(key)
}

/// Whether a name gets its reading drawn beside it. The last test is the
/// fold rather than `is_ascii`, because Beyoncé and Straße fold back to
/// Latin letters and never get a reading; the `is_ascii` in front spares
/// the Latin library the fold's allocation. The tests run in cost order.
pub fn shows_reading(name: &str, reading: &str, show: bool) -> bool {
    show && !reading.is_empty()
        && reading != name
        && !name.is_ascii()
        && !rox_library::fold::fold(name).is_ascii()
}

/// A name with its reading after it, "秋ノ風 (Aki no kaze)". One text
/// element, so the pair truncates as one line and the parenthesis can't
/// wrap alone; the reading is a faint highlight over the trailing span.
pub fn named(name: &str, reading: &str, show: bool) -> impl IntoElement + use<> {
    if !shows_reading(name, reading, show) {
        return StyledText::new(SharedString::from(name.to_owned()));
    }
    let mut text = String::with_capacity(name.len() + reading.len() + 3);
    text.push_str(name);
    text.push_str(" (");
    text.push_str(reading);
    text.push(')');
    // One step below muted: at muted the reading still reads as part of the title.
    let faint = name.len()..text.len();
    StyledText::new(SharedString::from(text)).with_highlights([(
        faint,
        HighlightStyle {
            color: Some(palette::text_faint().into()),
            ..Default::default()
        },
    )])
}

/// A panel whose per-view config is edited in the panel settings window:
/// the shared Appearance, Behavior and Shader pages, then its own pages.
pub trait PanelSettings: Panel {
    /// So the settings window can back itself with the playing track's art.
    fn state(&self) -> AppState;

    /// The panel's own pages as (name, sidebar icon), below the shared ones.
    /// The name is an identifier [`page`](Self::page) dispatches on; the
    /// sidebar shows [`page_label`] of it.
    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Whether the shared surface-shader page shows. A panel whose body
    /// already is a shader opts out.
    fn surface_shader(&self) -> bool {
        true
    }

    /// One of the panel's own pages. Changes apply live; the layout dump persists them.
    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (page, window, cx);
        div().into_any_element()
    }

    /// The shared knobs read and write through here rather than a method per field.
    fn chrome(&self) -> &PanelChrome;

    fn chrome_mut(&mut self) -> &mut PanelChrome;

    fn custom_title(&self) -> Option<&str> {
        self.chrome().title.as_deref()
    }

    /// None goes back to the built-in name. Implementations must repaint their
    /// hosting tab panel ([`refresh_tab_panel`]), which draws the title.
    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>);

    /// Whether the panel draws its own font control, so the Appearance page
    /// leaves off the generic font row. The lyrics panel does.
    fn has_own_font(&self) -> bool {
        false
    }

    fn theme(&self) -> PanelTheme {
        self.chrome().theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.chrome_mut().theme = theme;
        cx.notify();
    }

    /// The dock reads the flag through [`Panel::locked`] on its next paint.
    /// Read the current value off `chrome().locked`, which sidesteps the name
    /// clash with the dock trait's `locked`.
    fn set_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().locked = on;
        cx.notify();
    }

    fn set_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().anchor = on;
        cx.notify();
    }

    /// Whether this panel hosts others and so draws corner slot controls. It
    /// gates the row that hides them.
    fn composite(&self) -> bool {
        false
    }

    fn set_hide_controls(&mut self, on: bool, cx: &mut Context<Self>) {
        self.chrome_mut().hide_controls = on;
        cx.notify();
    }

    /// The dock re-reads the cap when it rebuilds the split's size range, so a
    /// repaint settles it. The other size setters work the same way.
    fn set_max_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_width = px;
        cx.notify();
    }

    fn set_max_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().max_height = px;
        cx.notify();
    }

    fn set_min_width(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_width = px;
        cx.notify();
    }

    fn set_min_height(&mut self, px: Option<f32>, cx: &mut Context<Self>) {
        self.chrome_mut().min_height = px;
        cx.notify();
    }

    /// The panel's own rows for the shared Appearance page, between the frame
    /// and the colors: looks stored on its config rather than its theme.
    fn appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }

    /// The panel's own rows for the shared Behavior page, under the placement and size rows.
    fn behavior(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let _ = (window, cx);
        None
    }
}

/// How far a press drags before an anchored panel starts a window move.
/// Under it the press stays a click, so an anchor over controls works like
/// a hidden macOS titlebar.
const ANCHOR_SLOP: Pixels = px(6.);

thread_local! {
    /// The pending anchor drag's window and press point. One pointer and one
    /// UI thread, so a thread local holds it.
    static ANCHOR_ARM: std::cell::Cell<Option<(gpui::WindowId, Point<Pixels>)>> =
        const { std::cell::Cell::new(None) };
}

/// The press passes through in capture phase, so the click still lands on
/// the control under it. The move starts once the pointer clears
/// [`ANCHOR_SLOP`].
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
            // An arm from another window, or one whose release this panel never saw,
            // dies here instead of hijacking a passing drag.
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
            // Keep the move from doubling as a text-selection drag underneath.
            cx.stop_propagation();
            window.start_window_move();
        })
        .capture_any_mouse_up(|_, _, _| ANCHOR_ARM.set(None))
        .on_mouse_up_out(MouseButton::Left, |_, _, _| ANCHOR_ARM.set(None))
}

/// Build a panel body under its palette override and keep the override
/// active through every element phase. Building under the scope covers the
/// eager style reads (`.bg(palette::x())`); the wrapper re-enters it for
/// layout, prepaint and paint, when hover styles and canvas closures read
/// the palette. Padding, rounding and border style the body's root div,
/// and margin wraps outside it so the backdrop shows through. The radius
/// sits on the body's own quad because gpui content masks stay rectangular.
pub fn themed(chrome: &PanelChrome, build: impl FnOnce() -> Div) -> AnyElement {
    let theme = &chrome.theme;
    let anchor = chrome.anchor;
    // Zero reads as no knob, so an explicit zero squares one panel off a
    // rounded app default.
    let app = rox_core::settings::app_frame();
    // Every knob goes through `positive`: a panel's knobs come from a layout
    // dump nobody sanitizes, and a negative inset would push it out of its cell.
    let margin = theme.margin.unwrap_or(app.margin).positive();
    let frame = {
        let padding = theme.padding.unwrap_or(app.padding).positive();
        let rounding = theme.rounding.unwrap_or(app.rounding);
        // Per side, an older config's edge mask folded in on the way.
        let border = theme.border_sides(app.border).positive();
        let font = theme.font.clone();
        move || {
            let mut body = build();
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
    // Anything that rounds to no change reads as follow-app.
    let rem_scale = theme
        .font_scale
        .map(|s| s.clamp(palette::PANEL_FONT_SCALE_MIN, palette::PANEL_FONT_SCALE_MAX))
        .filter(|s| (s - 1.0).abs() > 0.001);
    // A surface shader rides the same wrapper: it needs the element's bounds
    // and a paint hook after the body.
    let surface = shader::PanelSurface::build(chrome, margin);
    if scope.is_none() && rem_scale.is_none() && surface.is_none() {
        return frame();
    }
    // Build under both channels so scoped colors and `scaled_px` bake in at
    // construction; the wrapper re-applies them in each render phase.
    let child = panel_env(scope.as_ref(), rem_scale, frame);
    Themed {
        scope,
        rem_scale,
        surface,
        child,
    }
    .into_any_element()
}

/// Run `f` under a panel's palette scope and rem scale. The build and all
/// three render phases go through here so they read the same values.
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

/// Keeps a panel's palette scope and font scale active through the render
/// phases. The scope re-applies through its thread-local channel. The font
/// scale goes down two rails: the window rem (text and the vendored table)
/// and [`palette::rem_scaled`] (hand-rolled rows built without a `Window`),
/// both off the same multiplier so they stay in step.
struct Themed {
    scope: Option<palette::Scope>,
    rem_scale: Option<f32>,
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
        // Layout resolves the rem, but `with_rem_size` is paint-only, so override
        // the base the way the window root does and put it back after. No override
        // is active here, so the base is the app size.
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
        // A surface that reads `mask` gets the body's paint bracketed as its span,
        // keyed by the panel entity like the region, so the renderer replays exactly
        // what this panel drew.
        if self
            .surface
            .as_ref()
            .is_some_and(shader::PanelSurface::wants_mask)
        {
            let instance = window.current_view().as_u64();
            window.with_shader_mask_span(instance, |window| {
                window.with_rem_size(rem, |window| {
                    panel_env(scope, rem_scale, || child.paint(window, cx));
                });
            });
        } else {
            window.with_rem_size(rem, |window| {
                panel_env(scope, rem_scale, || child.paint(window, cx));
            });
        }
        // Post-order: the body is in the scene before the shader records, so a
        // screen pass samples it. A shaded child in a shaded host composes first.
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

/// Longer than the transport panel's ten: fifteen is about how long it
/// takes to hear whether a band or a curve did what was wanted.
const STRIP_SEEK: f64 = 15.0;

/// Back fifteen, play/pause, forward fifteen, random, for the windows that
/// aren't the workspace but still need playback in reach (EQ, signals,
/// output). Nudges replace the track buttons because a skip would throw
/// away the passage being judged. No title either: one that grows with the
/// track would shift the buttons out from under the pointer.
///
/// The caller has to keep the view awake with a `cx.observe(&player, ...)`,
/// or the play/pause face goes stale when a track ends on its own.
pub fn transport_strip<P: 'static>(
    player: &Entity<Player>,
    library: &Entity<Library>,
    cx: &mut Context<P>,
) -> Div {
    let random = {
        let player = player.clone();
        let library = library.clone();
        rox_panel_kit::ui::icon_button(icons::DICE, false, move |_, _, cx| {
            player.update(cx, |player, cx| player.play_random(&library, cx));
        })
    };
    transport_nudges(player, cx).child(random)
}

/// The strip without the die, for a window that already chose what plays
/// (the genre tagger), where a random draw would swap the track out.
pub fn transport_nudges<P: 'static>(player: &Entity<Player>, cx: &mut Context<P>) -> Div {
    let playing = player.read(cx).is_playing();
    let button = |icon: &'static str,
                  player: Entity<Player>,
                  verb: fn(&mut Player, &mut Context<Player>)| {
        rox_panel_kit::ui::icon_button(icon, false, move |_, _, cx| player.update(cx, verb))
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
}

/// A panel in a window of its own. Right-click serves the panel's own menu,
/// the one its tab drops down in the dock.
struct PopoutHost {
    panel_view: Arc<dyn PanelView>,
    state: AppState,
    backdrop: WindowBackdrop,
    /// The anchor, the menu, and the dismiss subscription that clears it.
    context_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Fallback focus, so the Workspace-scoped playback bindings have a
    /// dispatch path before the panel takes focus.
    focus: FocusHandle,
    window_id: u64,
    _backdrop_changed: Subscription,
}

impl Drop for PopoutHost {
    fn drop(&mut self) {
        PANEL_WINDOWS.write().unwrap().remove(&self.window_id);
    }
}

impl PopoutHost {
    /// The panel's own dropdown, everything its tab offers in the dock.
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
        // Renders under the parent player's tint, and claims the widget theme
        // while it holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                // Same Workspace context and playback actions as the main window. The
                // panel's own SearchInput context still carves the keys back.
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
                // No tab group here to answer the Panel Settings chord, so the host does.
                .on_action(cx.listener(|this, _: &OpenPanelSettings, window, cx| {
                    this.panel_view.open_settings(window, cx);
                }))
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // A panel with its own content menu already ends it with the panel tail,
                // so only panels without one get the window's right-click.
                .when(!self.panel_view.content_context_menu(cx), |body| {
                    body.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.open_menu(event.position, window, cx);
                        }),
                    )
                })
                // Under the panel; how much shows through is the surfaces' call (ADR 10).
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(self.panel_view.view())
                // Same overlay as the dock's context menu: an occluding layer swallows the
                // dismissing click.
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
        // A layout written before panel shaders existed.
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

        // A sane pair is left as written, and an unset cap stays unbounded.
        let sane = PanelChrome {
            min_width: Some(200.),
            max_width: Some(600.),
            min_height: Some(300.),
            ..PanelChrome::default()
        };
        assert_eq!(chrome_max_size(&sane, floor).width, px(600.));
        assert_eq!(chrome_max_size(&sane, floor).height, Pixels::MAX);
    }

    #[test]
    fn an_explicit_min_goes_under_the_floor() {
        let floor = gpui::size(px(120.), px(80.));
        let compact = PanelChrome {
            min_width: Some(40.),
            max_width: Some(60.),
            min_height: Some(12.),
            ..PanelChrome::default()
        };
        assert_eq!(chrome_min_size(&compact, floor).width, px(40.));
        assert_eq!(chrome_min_size(&compact, floor).height, px(12.));
        assert_eq!(chrome_max_size(&compact, floor).width, px(60.));

        assert_eq!(
            chrome_min_size(&PanelChrome::default(), floor),
            gpui::size(px(120.), px(80.))
        );

        let negative = PanelChrome {
            min_height: Some(-20.),
            ..PanelChrome::default()
        };
        assert_eq!(chrome_min_size(&negative, floor).height, px(0.));
    }
}

#[cfg(test)]
mod naming_tests {
    use super::shows_reading;

    #[test]
    fn a_latin_name_never_takes_a_reading() {
        assert!(!shows_reading("USAO", "USAO", true));
        assert!(!shows_reading("Beyoncé", "Beyonce", true));
        assert!(!shows_reading("Straße", "Strasse", true));
        assert!(!shows_reading("Sigur Rós", "Sigur Ros", true));
    }

    #[test]
    fn a_non_latin_name_takes_its_reading() {
        assert!(shows_reading("秋ノ風", "Aki no kaze", true));
        assert!(shows_reading("米津玄師", "Yonezu Kenshi", true));
        assert!(shows_reading("Мельница", "Melnitsa", true));
    }

    #[test]
    fn a_reading_that_adds_nothing_is_dropped() {
        assert!(!shows_reading("秋ノ風", "秋ノ風", true));
        assert!(!shows_reading("秋ノ風", "", true));
    }

    #[test]
    fn the_switch_turns_every_reading_off() {
        assert!(!shows_reading("秋ノ風", "Aki no kaze", false));
        assert!(!shows_reading("米津玄師", "Yonezu Kenshi", false));
    }
}
