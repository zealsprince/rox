//! The biography panel: who the current track's artist is. A wide image
//! banner with the name, country, and years active over its foot, then the
//! listening stats, genre tags, wiki text, top tracks, and similar names,
//! over the artist fanart dimmed into the background. It all comes from the
//! artist store's cached fetches, so a shown artist reads offline from then
//! on. A tag crediting several acts splits into chips, one sheet per act,
//! and a similar name is a chip that turns the sheet to that artist.
//!
//! Which track is per-view config through [`crate::source::TrackSource`], so
//! a duplicate can watch each.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    ObjectFit, Rgba, ScrollHandle, SharedString, Stateful, Subscription, WeakEntity, Window, div,
    img, linear_color_stop, linear_gradient, point, prelude::*, px, svg,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::text::{TextView, TextViewStyle};
use gpui_component::{Icon, Sizable, Size};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::FilterField;
use rox_net::providers::lastfm::{BioLink, TopTrack};
use rox_net::providers::theaudiodb::ArtistProfile;
use serde::{Deserialize, Serialize};

use crate::artists::{self, Artist, SizedImage};
use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::providers;
use crate::query::shared_query::{self, SharedQuery};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::source::{self, ResolvedTrack, TrackSource};

/// Header band height in px. The ceiling only stops a stray digit making a
/// band taller than any screen.
const HEADER_H_DEFAULT: f32 = 200.;
const HEADER_H_MIN: f32 = 40.;
const HEADER_H_MAX: f32 = 4000.;

const HEADER_ARROW: f32 = 28.;

const OVERLAY_SCRIM_H: f32 = 56.;

const BACKGROUND_OPACITY_DEFAULT: f32 = 40.;

/// Slower than the palette's ease: a picture swapping under a title reads
/// better slow.
const HEADER_FADE_SECS: f32 = 0.8;

const CYCLE_TICK: Duration = Duration::from_secs(1);

/// Reserved on every top track row, icons or not, so the ranks line up.
const TRACK_ACTIONS_W: f32 = 36.;

const TOP_TRACK_COUNTS: [usize; 3] = [3, 5, 10];

const CYCLE_INTERVALS: [u64; 5] = [5, 10, 20, 30, 60];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BiographyConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: TrackSource,
    pub name_source: NameSource,
    /// The header image. Named `portrait` from before the banner existed;
    /// renaming it breaks saved layouts.
    pub portrait: bool,
    /// Keep the header image at its own proportions; off crops it into a
    /// fixed band.
    pub header_aspect: bool,
    /// Let a tall header image span the full width uncapped. Only applies
    /// while the proportions are kept.
    pub header_fill: bool,
    /// With the proportions off, fit the whole image over a blurred copy of
    /// itself instead of cropping.
    pub header_blur: bool,
    /// The band's height, or its cap while the proportions are kept.
    pub header_height: f32,
    pub header_overlay: bool,
    /// theaudiodb's logo strip, off by default: a 1000x185 lettering band
    /// reads poorly under a title.
    pub header_banner: bool,
    pub header_fanart: bool,
    pub cycle: bool,
    pub cycle_secs: u64,
    pub background: bool,
    pub background_opacity: f32,
    /// The country and years active line.
    pub profile: bool,
    /// The country as its flag; off, as its two-letter code.
    pub flag: bool,
    pub stats: bool,
    pub tags: bool,
    pub top_tracks: bool,
    pub top_tracks_count: usize,
    pub similar: bool,
}

impl Default for BiographyConfig {
    fn default() -> Self {
        BiographyConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            name_source: NameSource::Artist,
            portrait: true,
            header_aspect: true,
            header_fill: false,
            header_blur: true,
            header_height: HEADER_H_DEFAULT,
            header_overlay: true,
            header_banner: false,
            header_fanart: true,
            cycle: true,
            cycle_secs: 10,
            background: false,
            background_opacity: BACKGROUND_OPACITY_DEFAULT,
            profile: true,
            flag: true,
            stats: true,
            tags: true,
            top_tracks: true,
            top_tracks_count: 5,
            similar: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    Artist,
    AlbumArtist,
}

pub struct BiographyPanel {
    state: AppState,
    config: BiographyConfig,
    /// The shown track's credits, cached because the pump notifies per
    /// frame. The `u64` is the station-title revision: a station's key
    /// stands still while the song under it turns over.
    artist: Option<(TrackKey, u64, Vec<String>)>,
    pick: usize,
    /// Names browsed into off the similar lists; empty means the sheet shows
    /// the credited artist.
    trail: Vec<String>,
    /// Every album artist, folded: the evidence that keeps "Earth, Wind &
    /// Fire" whole when splitting credits.
    known_acts: Option<Arc<HashSet<String>>>,
    /// Every name the library files tracks under, folded, with the field and
    /// spelling a search pick wants.
    held: Option<Arc<HashMap<String, (FilterField, String)>>>,
    /// Keyed by the folded name; None inside is a clean miss.
    loaded: Option<(String, Option<Artist>)>,
    pending: Option<String>,
    error: Option<(String, SharedString)>,
    resolved: ResolvedTrack,
    /// Discards stale fetch results when the artist changes mid-flight.
    generation: u64,
    header_ix: usize,
    fade: Option<(SizedImage, Instant)>,
    advanced_at: Instant,
    /// Library ids for the top tracks, keyed by the folded artist; None is a
    /// track the library doesn't hold.
    matches: Option<(String, Vec<Option<i64>>)>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    opacity_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    height_input: Option<(Entity<InputState>, Subscription)>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
    /// Without it a closed panel leaves its images pinned in gpui's
    /// never-evicting asset cache.
    _retire_on_drop: Subscription,
}

impl BiographyPanel {
    pub fn new(state: AppState, config: BiographyConfig, cx: &mut Context<Self>) -> Self {
        // Gated on the view and the station-title revision: a station's view
        // doesn't move when the stream announces its next song.
        let _player_changed = {
            let mut last = {
                let player = state.player.read(cx);
                (player.view(), player.title_rev())
            };
            cx.observe(&state.player, move |_: &mut Self, player, cx| {
                let now = {
                    let player = player.read(cx);
                    (player.view(), player.title_rev())
                };
                if now != last {
                    last = now;
                    cx.notify();
                }
            })
        };
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags and id mappings. The store's results
        // stay, since they key on the name.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.artist = None;
                this.known_acts = None;
                this.held = None;
                this.matches = None;
                cx.notify();
            },
        );
        // Taking loaded first leaves nothing held, so retire drops every
        // image the panel still had on screen.
        let _retire_on_drop = cx.on_release(|this, cx| {
            this.fade = None;
            let old = this.loaded.take().and_then(|(_, a)| a);
            this.retire(old, cx);
        });
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(CYCLE_TICK).await;
                if view.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        BiographyPanel {
            state,
            config,
            artist: None,
            pick: 0,
            trail: Vec::new(),
            known_acts: None,
            held: None,
            loaded: None,
            pending: None,
            error: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            header_ix: 0,
            fade: None,
            advanced_at: Instant::now(),
            matches: None,
            scroll: ScrollHandle::default(),
            focus: cx.focus_handle().tab_stop(true),
            opacity_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            height_input: None,
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
            _retire_on_drop,
        }
    }

    /// Credits off the chosen tag, the other tag standing in when it's
    /// empty. A station's read goes through the player's announced song.
    fn credits_for(&mut self, key: &TrackKey, cx: &App) -> Vec<String> {
        let rev = self.live_rev(key, cx);
        if self.artist.as_ref().map(|(k, r, _)| (k, *r)) != Some((key, rev)) {
            let known = self.known_acts(cx);
            let names = self
                .live_meta(key, cx)
                .map(|meta| {
                    let (first, second) = match self.config.name_source {
                        NameSource::Artist => (meta.artist, meta.album_artist),
                        NameSource::AlbumArtist => (meta.album_artist, meta.artist),
                    };
                    let is_known = |name: &str| known.contains(&fold_name(name));
                    let names = credits(&first, &is_known);
                    if names.is_empty() {
                        credits(&second, &is_known)
                    } else {
                        names
                    }
                })
                .unwrap_or_default();
            self.artist = Some((key.clone(), rev, names));
            self.pick = 0;
            self.trail.clear();
        }
        self.artist
            .as_ref()
            .map(|(.., names)| names.clone())
            .unwrap_or_default()
    }

    /// Zero unless the shown track is the live stream playing.
    fn live_rev(&self, key: &TrackKey, cx: &App) -> u64 {
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.live && now.key == *key => player.title_rev().unwrap_or(0),
            _ => 0,
        }
    }

    /// A stream's row names the station, so the credits come off the
    /// announced song laid over it.
    fn live_meta(&self, key: &TrackKey, cx: &App) -> Option<rox_library::store::TrackMeta> {
        let row = self.state.library.read(cx).meta_for_key(key);
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.key == *key => player.live_over(row),
            _ => row,
        }
    }

    /// An album is filed under the act that made it, so a name with a comma
    /// that shows up as an album artist is one act.
    fn known_acts(&mut self, cx: &App) -> Arc<HashSet<String>> {
        if let Some(known) = &self.known_acts {
            return known.clone();
        }
        let library = self.state.library.read(cx);
        let known: HashSet<String> = library
            .projection()
            .map(|projection| {
                projection
                    .album_artists
                    .strings
                    .iter()
                    .filter(|name| !name.is_empty())
                    .map(|name| fold_name(name))
                    .collect()
            })
            .unwrap_or_default();
        let known = Arc::new(known);
        self.known_acts = Some(known.clone());
        known
    }

    fn picked(&self) -> String {
        self.artist
            .as_ref()
            .and_then(|(.., names)| names.get(self.pick).or_else(|| names.first()))
            .cloned()
            .unwrap_or_default()
    }

    fn shown(&self) -> String {
        self.trail.last().cloned().unwrap_or_else(|| self.picked())
    }

    /// A name already on the trail steps back to it rather than looping.
    fn browse(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(at) = self.trail.iter().position(|n| *n == name) {
            self.trail.truncate(at + 1);
        } else if fold_name(&name) == fold_name(&self.picked()) {
            self.trail.clear();
        } else {
            self.trail.push(name);
        }
        cx.notify();
    }

    /// The chosen tag's table goes first, so a name in both picks the field
    /// the sheet reads.
    fn held(&mut self, cx: &App) -> Arc<HashMap<String, (FilterField, String)>> {
        if let Some(held) = &self.held {
            return held.clone();
        }

        let library = self.state.library.read(cx);
        let held = library
            .projection()
            .map(|projection| {
                let artists = (FilterField::Artist, projection.artists.strings.as_slice());
                let album_artists = (
                    FilterField::AlbumArtist,
                    projection.album_artists.strings.as_slice(),
                );
                match self.config.name_source {
                    NameSource::Artist => held_index([artists, album_artists]),
                    NameSource::AlbumArtist => held_index([album_artists, artists]),
                }
            })
            .unwrap_or_default();
        let held = Arc::new(held);
        self.held = Some(held.clone());
        held
    }

    /// `force` refetches past the store's TTL.
    fn ensure_loaded(&mut self, name: &str, force: bool, cx: &mut Context<Self>) {
        let key = providers::normalize(name);
        if !force
            && (self.loaded.as_ref().is_some_and(|(k, _)| *k == key)
                || self.pending.as_deref() == Some(&key)
                || self.error.as_ref().is_some_and(|(k, _)| *k == key))
        {
            return;
        }
        self.pending = Some(key.clone());
        self.error = None;
        self.generation += 1;
        let generation = self.generation;
        let name = name.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let name = name.clone();
                    async move { artists::get(&name, force) }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(artist) => {
                        // No fade may keep a retired image on screen.
                        this.header_ix = 0;
                        this.fade = None;
                        this.advanced_at = Instant::now();
                        this.matches = None;
                        let old = this.loaded.take().and_then(|(_, a)| a);
                        this.loaded = Some((key, artist));
                        this.retire(old, cx);
                    }
                    Err(e) => {
                        log::warn!("biography: {name}: {e}");
                        this.error = Some((key, format!("Couldn't load {name}: {e}").into()))
                    }
                }
                this.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// gpui's asset cache never evicts, so every artist viewed would leak its
    /// images without this. Skips any the fresh artist still shows.
    fn retire(&self, old: Option<Artist>, cx: &mut App) {
        let Some(old) = old else { return };
        let kept = self
            .loaded
            .as_ref()
            .and_then(|(_, artist)| artist.as_ref())
            .map(Artist::images)
            .unwrap_or_default();
        for image in old.images() {
            if !kept.iter().any(|keep| keep.id() == image.id()) {
                image.remove_asset(cx);
            }
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let name = self.shown();
        if name.is_empty() {
            return;
        }
        self.ensure_loaded(&name, true, cx);
        cx.notify();
    }

    fn headers(&self, artist: &Artist) -> Vec<SizedImage> {
        let mut list = Vec::new();
        if self.config.header_banner {
            list.extend(artist.banner.clone());
        }
        if self.config.header_fanart {
            list.extend(artist.fanarts.iter().cloned());
        }
        if list.is_empty() {
            list.extend(artist.portrait.clone());
        } else if !self.config.cycle {
            list.truncate(1);
        }
        list
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        if !self.config.cycle || self.advanced_at.elapsed().as_secs() < self.config.cycle_secs {
            return;
        }
        self.step(1, cx);
    }

    /// Restarts the cycle's clock so a hand-picked image gets its full
    /// interval.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((_, Some(artist))) = &self.loaded else {
            return;
        };
        let headers = self.headers(artist);
        if headers.len() < 2 {
            return;
        }
        let len = headers.len();
        let from = headers[self.header_ix % len].clone();
        self.header_ix = (self.header_ix % len)
            .wrapping_add_signed(delta)
            .rem_euclid(len);
        self.fade = Some((from, Instant::now()));
        self.advanced_at = Instant::now();
        cx.notify();
    }

    fn height_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some((input, _)) = &self.height_input {
            return input.clone();
        }
        let current = format!("{}", self.config.header_height.round() as i64);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let events = cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let Ok(value) = input.read(cx).value().trim().parse::<f32>() else {
                return;
            };
            this.config.header_height = value.clamp(HEADER_H_MIN, HEADER_H_MAX);
            cx.notify();
        });
        self.height_input = Some((input.clone(), events));
        input
    }

    /// Folds with the search's case and accent fold plus the provider's
    /// punctuation fold, so "Don't" against "Don’t" still meets.
    fn matches_for(
        &mut self,
        key: &str,
        lastfm_name: &str,
        tracks: &[TopTrack],
        cx: &App,
    ) -> Vec<Option<i64>> {
        if let Some((k, matches)) = &self.matches
            && k == key
        {
            return matches.clone();
        }
        let fold = |text: &str| providers::normalize(&rox_library::fold::fold(text));
        let mut names: Vec<String> = vec![fold(lastfm_name)];
        let shown = fold(&self.shown());
        if !names.contains(&shown) {
            names.push(shown);
        }
        names.retain(|name| !name.is_empty());
        let titles: Vec<String> = tracks.iter().map(|track| fold(&track.name)).collect();
        let mut matches: Vec<Option<i64>> = vec![None; tracks.len()];
        let known = self.known_acts(cx);
        let is_known = |name: &str| known.contains(&fold_name(name));
        let library = self.state.library.read(cx);
        if let Some(projection) = library.projection() {
            let mut open = titles.iter().filter(|t| !t.is_empty()).count();
            for row in 0..projection.len() as u32 {
                if open == 0 {
                    break;
                }
                if projection.is_dead(row) {
                    continue;
                }
                let view = projection.resolve(row);
                let by_this_artist = credits(view.artist, &is_known)
                    .iter()
                    .chain(credits(view.album_artist, &is_known).iter())
                    .any(|credit| names.contains(&fold(credit)));
                if !by_this_artist {
                    continue;
                }
                let title = fold(view.title);
                if title.is_empty() {
                    continue;
                }
                for (i, wanted) in titles.iter().enumerate() {
                    if matches[i].is_none() && *wanted == title {
                        matches[i] = Some(projection.db_id[row as usize]);
                        open -= 1;
                    }
                }
            }
        }
        self.matches = Some((key.to_string(), matches.clone()));
        matches
    }

    /// Selects without playing, the way a click in the library does.
    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(vec![id], source, cx));
        cx.notify();
    }

    fn play(&mut self, id: i64, queue: bool, cx: &mut Context<Self>) {
        let Ok(keys) = self.state.library.read(cx).keys_for(&[id]) else {
            return;
        };
        if keys.is_empty() {
            return;
        }
        self.state.player.update(cx, |player, cx| {
            if queue {
                player.enqueue(keys, cx);
            } else {
                player.play_now(keys, cx);
            }
        });
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = source::source_flyout(
            menu,
            |this: &Self| this.config.source,
            &cx.entity(),
            |this, source, cx| {
                this.config.source = source;
                cx.notify();
            },
            window,
            cx,
        );
        // No icon: the left-side check shows the state, and an icon would
        // take that slot.
        let entity = cx.entity();
        let toggle =
            |menu: PopupMenu, label: SharedString, checked, set: fn(&mut BiographyConfig)| {
                let weak = entity.downgrade();
                menu.item(
                    PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            this.update(cx, |this, cx| {
                                set(&mut this.config);
                                cx.notify();
                            });
                        }),
                )
            };
        let menu = toggle(
            menu,
            rox_i18n::t!("biography-header-image"),
            self.config.portrait,
            |c| c.portrait = !c.portrait,
        );
        let menu = toggle(
            menu,
            rox_i18n::t!("biography-keep-aspect"),
            self.config.header_aspect,
            |c| c.header_aspect = !c.header_aspect,
        );
        let menu = toggle(
            menu,
            rox_i18n::t!("biography-fill-width"),
            self.config.header_fill,
            |c| c.header_fill = !c.header_fill,
        );
        let menu = toggle(
            menu,
            rox_i18n::t!("biography-cycle"),
            self.config.cycle,
            |c| c.cycle = !c.cycle,
        );
        let menu = toggle(
            menu,
            rox_i18n::t!("biography-background"),
            self.config.background,
            |c| c.background = !c.background,
        );
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new(rox_i18n::t!("biography-refresh"))
                .icon(Icon::default().path(icons::REFRESH_CW))
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.refresh(cx));
                }),
        )
    }
}

impl PanelSettings for BiographyPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Content", icons::USER)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let height_input = self.height_input(window, cx);
        let flag =
            |label: &str, on: bool, set: fn(&mut BiographyConfig, bool), cx: &mut Context<Self>| {
                panel::setting_row(
                    rox_i18n::t!(label),
                    Some(rox_i18n::t!(&format!("{label}.description"))),
                    panel::toggle(
                        on,
                        move |this: &mut Self, on, cx| {
                            set(&mut this.config, on);
                            cx.notify();
                        },
                        cx,
                    ),
                )
            };
        let intervals: Vec<(SharedString, u64)> = CYCLE_INTERVALS
            .iter()
            .map(|secs| {
                (
                    rox_i18n::t!("biography-seconds", count = secs.to_string()),
                    *secs,
                )
            })
            .collect();
        let counts: Vec<(SharedString, usize)> = TOP_TRACK_COUNTS
            .iter()
            .map(|n| (SharedString::from(n.to_string()), *n))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(source::source_row(
                self.config.source,
                |this: &mut Self, source, cx| {
                    this.config.source = source;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                rox_i18n::t!("biography-name-source"),
                Some(rox_i18n::t!("biography-name-source.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("head-piece-artist"), NameSource::Artist),
                        (
                            rox_i18n::t!("filter-field-album-artist"),
                            NameSource::AlbumArtist,
                        ),
                    ],
                    self.config.name_source,
                    |this: &mut Self, name_source, cx| {
                        this.config.name_source = name_source;
                        this.artist = None;
                        this.held = None;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(flag(
                "biography-header-image",
                self.config.portrait,
                |c, on| c.portrait = on,
                cx,
            ))
            .child(flag(
                "biography-keep-aspect",
                self.config.header_aspect,
                |c, on| c.header_aspect = on,
                cx,
            ))
            .when(self.config.header_aspect, |d| {
                d.child(flag(
                    "biography-fill-width",
                    self.config.header_fill,
                    |c, on| c.header_fill = on,
                    cx,
                ))
            })
            .when(!self.config.header_aspect, |d| {
                d.child(flag(
                    "biography-header-blur",
                    self.config.header_blur,
                    |c, on| c.header_blur = on,
                    cx,
                ))
            })
            // With the proportions kept and the fill on there's no cap, so the
            // row would set nothing.
            .when(
                !(self.config.header_aspect && self.config.header_fill),
                |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("biography-header-height"),
                        Some(rox_i18n::t!("biography-header-height.description")),
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_XS)
                            .child(div().w(px(72.)).child(Input::new(&height_input).small()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child("px"),
                            ),
                    ))
                },
            )
            .child(flag(
                "biography-header-overlay",
                self.config.header_overlay,
                |c, on| c.header_overlay = on,
                cx,
            ))
            .child(flag(
                "biography-header-banner",
                self.config.header_banner,
                |c, on| c.header_banner = on,
                cx,
            ))
            .child(flag(
                "biography-header-fanart",
                self.config.header_fanart,
                |c, on| c.header_fanart = on,
                cx,
            ))
            .child(flag(
                "biography-cycle",
                self.config.cycle,
                |c, on| c.cycle = on,
                cx,
            ))
            .when(self.config.cycle, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("biography-cycle-interval"),
                    Some(rox_i18n::t!("biography-cycle-interval.description")),
                    panel::choices_shared(
                        &intervals,
                        self.config.cycle_secs,
                        |this: &mut Self, secs, cx| {
                            this.config.cycle_secs = secs;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(flag(
                "biography-background",
                self.config.background,
                |c, on| c.background = on,
                cx,
            ))
            .when(self.config.background, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("biography-background-opacity"),
                    Some(rox_i18n::t!("biography-background-opacity.description")),
                    settings_ui::scalar(
                        &self.opacity_scrub,
                        &self.value_edit,
                        self.config.background_opacity,
                        settings_ui::span(0., 100., "%").hard(),
                        |this: &mut Self, value, cx| {
                            this.config.background_opacity = value;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(flag(
                "biography-profile",
                self.config.profile,
                |c, on| c.profile = on,
                cx,
            ))
            .when(self.config.profile, |d| {
                d.child(flag(
                    "biography-flag",
                    self.config.flag,
                    |c, on| c.flag = on,
                    cx,
                ))
            })
            .child(flag(
                "biography-stats",
                self.config.stats,
                |c, on| c.stats = on,
                cx,
            ))
            .child(flag(
                "biography-tags",
                self.config.tags,
                |c, on| c.tags = on,
                cx,
            ))
            .child(flag(
                "biography-top-tracks",
                self.config.top_tracks,
                |c, on| c.top_tracks = on,
                cx,
            ))
            .when(self.config.top_tracks, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("biography-top-tracks-count"),
                    Some(rox_i18n::t!("biography-top-tracks-count.description")),
                    panel::choices_shared(
                        &counts,
                        self.config.top_tracks_count,
                        |this: &mut Self, n, cx| {
                            this.config.top_tracks_count = n;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(flag(
                "biography-similar-artists",
                self.config.similar,
                |c, on| c.similar = on,
                cx,
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for BiographyPanel {}

impl Focusable for BiographyPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for BiographyPanel {
    fn panel_name(&self) -> &'static str {
        "biography"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("biography-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = self.config_menu(menu, window, cx);
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                BiographyPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for BiographyPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // A focus stop, which also puts the tab group on the focus path for
        // the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl BiographyPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The floor at surface opacity, so the window backdrop (ADR 10) shows
        // through.
        let root = div().size_full().bg(palette::bg_root());
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return root.child(quiet(rox_i18n::t!("content-no-track")));
        };
        let names = self.credits_for(&key, cx);
        let name = self.shown();
        if name.is_empty() {
            return root.child(quiet(rox_i18n::t!("biography-no-artist-tag")));
        }
        self.ensure_loaded(&name, false, cx);
        let key = providers::normalize(&name);
        // A chip per credited act, then the browse trail behind chevrons. The
        // one shown is in the accent.
        let trail = self.trail.clone();
        let picker = (names.len() > 1 || !trail.is_empty()).then(|| {
            let pick = self.pick.min(names.len().saturating_sub(1));
            let browsing = !trail.is_empty();
            let mut row = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(tokens::SPACE_XS)
                .p(tokens::SPACE_SM)
                .children(names.iter().enumerate().map(|(i, credit)| {
                    let picked = i == pick && !browsing;
                    chip(credit.clone())
                        .id(("biography-credit", i))
                        .cursor_pointer()
                        .when(picked, |d| {
                            d.bg(palette::accent())
                                .text_color(palette::text_on_accent())
                        })
                        .when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick = i;
                            this.trail.clear();
                            cx.notify();
                        }))
                }));

            for (i, name) in trail.iter().enumerate() {
                let last = i + 1 == trail.len();
                row = row
                    .child(
                        svg()
                            .path(icons::CHEVRON_RIGHT)
                            .size(px(12.))
                            .flex_none()
                            .text_color(palette::text_faint()),
                    )
                    .child(
                        chip(name.clone())
                            .id(("biography-trail", i))
                            .cursor_pointer()
                            .when(last, |d| {
                                d.bg(palette::accent())
                                    .text_color(palette::text_on_accent())
                            })
                            .when(!last, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.trail.truncate(i + 1);
                                cx.notify();
                            })),
                    );
            }
            row
        });
        let root = root.flex().flex_col();
        let root = root.children(picker);
        match &self.loaded {
            Some((k, Some(artist))) if *k == key => {
                let artist = artist.clone();
                root.child(self.sheet(&artist, &key, window, cx))
            }
            Some((k, None)) if *k == key => root.child(quiet(rox_i18n::t!(
                "biography-not-found",
                name = name.clone()
            ))),
            _ => match &self.error {
                Some((k, error)) if *k == key => {
                    root.child(rox_panel_api::openers::console_notice(error.clone()))
                }
                _ => root.child(loading(rox_i18n::t!(
                    "biography-looking-up",
                    name = name.clone()
                ))),
            },
        }
    }

    /// The frame the header image covers: a fixed band with the proportions
    /// off, full width at the image's ratio with the fill on, and that ratio
    /// capped at the height otherwise.
    fn header_band(&self, ratio: f32) -> Div {
        let band = div().w_full().flex_none().overflow_hidden().relative();
        let height = self.config.header_height.clamp(HEADER_H_MIN, HEADER_H_MAX);
        if !self.config.header_aspect {
            return band.h(px(height));
        }
        let mut band = if self.config.header_fill {
            band
        } else {
            band.max_h(px(height))
        };
        band.style().aspect_ratio = Some(ratio);
        band
    }

    fn header_layer(&self, sized: &SizedImage) -> Div {
        let fit = !self.config.header_aspect && self.config.header_blur;
        let mut layer = div().absolute().inset_0();
        if fit && let Some(soft) = &sized.soft {
            layer = layer.child(
                div().absolute().inset_0().child(
                    img(soft.clone())
                        .overflow_hidden()
                        .object_fit(ObjectFit::Cover)
                        .size_full(),
                ),
            );
        }
        layer.child(
            img(sized.image.clone())
                .overflow_hidden()
                .object_fit(if fit {
                    ObjectFit::Contain
                } else {
                    ObjectFit::Cover
                })
                .size_full(),
        )
    }

    fn header(
        &mut self,
        artist: &Artist,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !self.config.portrait {
            return None;
        }
        let headers = self.headers(artist);
        if headers.is_empty() {
            return None;
        }
        let current = headers[self.header_ix % headers.len()].clone();
        // Frames only while a fade runs, smoothstepped to ease out.
        let mut opacity = 1.0;
        let mut outgoing = None;
        if let Some((from, at)) = self.fade.clone() {
            let u = (at.elapsed().as_secs_f32() / HEADER_FADE_SECS).min(1.0);
            if u < 1.0 {
                window.request_animation_frame();
                opacity = u * u * (3.0 - 2.0 * u);
                outgoing = Some(from);
            } else {
                self.fade = None;
            }
        }
        let mut band = self.header_band(current.ratio);
        if let Some(from) = outgoing {
            band = band.child(self.header_layer(&from));
        }
        band = band.child(self.header_layer(&current).opacity(opacity));
        if headers.len() > 1 {
            let arrow = |side: &'static str, icon: &'static str, delta: isize| {
                let base = palette::bg_root_opaque();
                div()
                    .absolute()
                    .top_1_2()
                    .mt(px(-HEADER_ARROW / 2.))
                    .map(|d| {
                        if side == "left" {
                            d.left_2()
                        } else {
                            d.right_2()
                        }
                    })
                    .size(px(HEADER_ARROW))
                    .rounded_full()
                    .bg(palette::alpha(base, 0xA6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover("biography-header", |d| d.opacity(1.))
                    .hover(|d| d.bg(palette::alpha(base, 0xE6)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.step(delta, cx);
                        }),
                    )
                    .child(
                        svg()
                            .path(icon)
                            .size(px(14.))
                            .text_color(palette::text_bright()),
                    )
            };
            band = band
                .group("biography-header")
                .child(arrow("left", icons::CHEVRON_LEFT, -1))
                .child(arrow("right", icons::CHEVRON_RIGHT, 1));
        }
        if self.config.header_overlay {
            let base = palette::bg_root();
            band = band.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .pt(px(OVERLAY_SCRIM_H))
                    // Angle 0 puts 0% at the bottom.
                    .bg(linear_gradient(
                        0.0,
                        linear_color_stop(scrim(base, 0xD9), 0.0),
                        linear_color_stop(scrim(base, 0x00), 1.0),
                    ))
                    .child(
                        self.title_block(artist)
                            .px(tokens::SPACE_MD)
                            .pb(tokens::SPACE_SM),
                    ),
            );
        }
        Some(band)
    }

    fn title_block(&self, artist: &Artist) -> Div {
        let profile = &artist.profile;
        let mut name = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .min_w_0();
        // The language picker draws flags the same way, so the glyphs are
        // known to land.
        if self.config.profile && !profile.country.is_empty() {
            let glyph = self.config.flag.then(|| flag(&profile.country)).flatten();
            name = name.child(match glyph {
                Some(flag) => div().text_lg().child(flag),
                None => chip(profile.country.clone()),
            });
        }
        name = name.child(
            div()
                .text_lg()
                .text_color(palette::text_bright())
                .child(SharedString::from(artist.info.name.clone())),
        );
        let mut block = div().flex().flex_col().gap(px(2.)).min_w_0().child(name);
        if self.config.profile
            && let Some(years) = years_active(profile)
        {
            block = block.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(years),
            );
        }
        block
    }

    fn sheet(
        &mut self,
        artist: &Artist,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let info = &artist.info;
        let mut column = div().flex().flex_col().w_full();
        let header = self.header(artist, window, cx);
        let overlaid = header.is_some() && self.config.header_overlay;
        if let Some(header) = header {
            column = column.child(header);
        }
        let mut content = div()
            .flex()
            .flex_col()
            .w_full()
            .p(tokens::SPACE_MD)
            .gap(tokens::SPACE_SM);
        if !overlaid {
            content = content.child(self.title_block(artist));
        }
        if self.config.stats && (info.listeners > 0 || info.playcount > 0) {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_MD)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "biography-listeners-count",
                        count = fmt_count(info.listeners)
                    ))
                    .child(rox_i18n::t!(
                        "biography-plays-count",
                        count = fmt_count(info.playcount)
                    )),
            );
        }
        if self.config.tags && !info.tags.is_empty() {
            // Clickable only while a search box is up somewhere, so a click
            // never narrows the followers with nothing on screen saying why.
            let query = self
                .state
                .query
                .read(cx)
                .has_box()
                .then(|| self.state.query.clone());
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(tokens::SPACE_XS)
                    .children(
                        info.tags
                            .iter()
                            .enumerate()
                            .map(|(i, tag)| tag_chip(i, tag.clone(), query.clone())),
                    ),
            );
        }
        if info.bio.is_empty() {
            content = content.child(
                div()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("biography-no-text")),
            );
        } else {
            // Escaped so the article can't format itself. Keyed by artist so
            // the view's selection doesn't carry over.
            let markdown = bio_markdown(&info.bio, info.links.as_deref().unwrap_or(&[]));
            content = content.child(
                div()
                    .mt(tokens::SPACE_XS)
                    .text_color(palette::text())
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("biography-bio-{key}")),
                            markdown,
                            window,
                            cx,
                        )
                        .selectable(true)
                        .style(TextViewStyle::default().paragraph_gap(gpui::rems(0.5))),
                    ),
            );
        }
        if let Some(list) = self.top_tracks(artist, key, cx) {
            content = content.child(list);
        }
        if self.config.similar && !info.similar.is_empty() {
            let held = self.held(cx);
            let query = self
                .state
                .query
                .read(cx)
                .has_box()
                .then(|| self.state.query.clone());
            let chips: Vec<Stateful<Div>> = info
                .similar
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let library = held.get(&fold_name(name)).cloned();
                    self.similar_chip(i, name.clone(), library, query.clone(), cx)
                })
                .collect();
            content = content.child(
                div()
                    .mt(tokens::SPACE_XS)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(heading(rox_i18n::t!("biography-similar-heading")))
                    .child(
                        div()
                            .mt(px(2.))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(tokens::SPACE_XS)
                            .children(chips),
                    ),
            );
        }
        // The attribution the wiki's license asks for.
        if !info.url.is_empty() {
            let url = info.url.clone();
            content = content.child(
                div().mt(tokens::SPACE_XS).text_xs().child(
                    div()
                        .text_color(palette::text_faint())
                        .hover(|d| d.text_color(palette::text_muted()))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| cx.open_url(&url))
                        .child(rox_i18n::t!("biography-from-lastfm")),
                ),
            );
        }
        let scroll = div()
            .id("biography-sheet")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(column.child(content));

        // The fanart is fixed to the panel, not the scroll, so it fades toward
        // the panel's bottom. flex_1 rather than size_full, since the credit
        // picker can sit above.
        let mut root = div().flex_1().min_h_0().w_full().relative();
        if self.config.background
            && let Some(image) = &artist.background
        {
            let base = palette::bg_root();
            let opacity = (self.config.background_opacity / 100.).clamp(0., 1.);
            root = root
                .child(
                    div().absolute().inset_0().opacity(opacity).child(
                        img(image.clone())
                            .overflow_hidden()
                            .object_fit(ObjectFit::Cover)
                            .size_full(),
                    ),
                )
                .child(div().absolute().inset_0().bg(linear_gradient(
                    0.0,
                    linear_color_stop(base, 0.0),
                    linear_color_stop(scrim(base, 0xA6), 1.0),
                )));
        }
        // gpui's overflow scroll draws no bar of its own.
        root.child(scroll).child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&self.scroll)),
        )
    }

    fn top_tracks(&mut self, artist: &Artist, key: &str, cx: &mut Context<Self>) -> Option<Div> {
        if !self.config.top_tracks {
            return None;
        }
        let tracks = artist.info.top_tracks.as_deref().unwrap_or(&[]);
        if tracks.is_empty() {
            return None;
        }
        let matches = self.matches_for(key, &artist.info.name, tracks, cx);
        let shown = tracks.len().min(self.config.top_tracks_count.max(1));
        let mut list = div()
            .mt(tokens::SPACE_XS)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(heading(rox_i18n::t!("biography-top-tracks-heading")));
        for (i, track) in tracks.iter().take(shown).enumerate() {
            let id = matches.get(i).copied().flatten();
            list = list.child(self.top_track_row(i, track, id, cx));
        }
        Some(list)
    }

    /// A row the library doesn't hold reads faint and is inert.
    fn top_track_row(
        &self,
        i: usize,
        track: &TopTrack,
        id: Option<i64>,
        cx: &mut Context<Self>,
    ) -> Div {
        let group: SharedString = format!("biography-top-track-{i}").into();
        let mut actions = div()
            .flex_none()
            .w(px(TRACK_ACTIONS_W))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.));
        let mut row = div()
            .group(group.clone())
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_XS)
            .py(px(2.))
            .rounded(tokens::RADIUS);
        let selected = id.is_some_and(|id| self.state.selection.read(cx).tracks() == [id]);
        if let Some(id) = id {
            let glyph = |path: &'static str| {
                svg()
                    .path(path)
                    .size(px(12.))
                    .text_color(palette::accent())
                    .cursor_pointer()
            };
            actions = actions
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .child(glyph(icons::PLAY).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.play(id, false, cx);
                    }),
                ))
                .child(glyph(icons::PLUS).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.play(id, true, cx);
                    }),
                ));
            row = row
                .cursor_pointer()
                .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
                .when(!selected, |d| {
                    d.hover(|d| d.bg(palette::bg_control_hover()))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        if event.click_count > 1 {
                            this.play(id, false, cx);
                        } else {
                            this.select(id, cx);
                        }
                    }),
                );
        }
        let title_color = if id.is_some() {
            palette::text()
        } else {
            palette::text_faint()
        };
        row.child(actions)
            .child(
                div()
                    .flex_none()
                    .w(px(18.))
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(format!("{}.", i + 1))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(title_color)
                    .child(SharedString::from(track.name.clone())),
            )
            .when(track.listeners > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(SharedString::from(fmt_count(track.listeners))),
                )
            })
    }

    /// A click browses to the name whether or not the library holds it. The
    /// search glyph picks the library's spelling so the filter's whole-value
    /// match lands.
    fn similar_chip(
        &self,
        i: usize,
        name: String,
        library: Option<(FilterField, String)>,
        query: Option<Entity<SharedQuery>>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let held = library.is_some();
        let browse = name.clone();
        let mut chip = chip(name)
            .id(("biography-similar", i))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .when(!held, |d| d.text_color(palette::text_faint()))
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.browse(browse.clone(), cx);
                }),
            );

        if let (Some((field, value)), Some(query)) = (library, query) {
            chip = chip.child(
                svg()
                    .path(icons::SEARCH)
                    .size(px(11.))
                    .flex_none()
                    .text_color(palette::accent())
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        shared_query::toggle_pick(&query, field, &value, cx);
                    }),
            );
        }
        chip
    }
}

fn bio_markdown(bio: &str, links: &[BioLink]) -> String {
    fn escape(text: &str, out: &mut String) {
        for ch in text.chars() {
            if matches!(
                ch,
                '\\' | '`'
                    | '*'
                    | '_'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '#'
                    | '+'
                    | '-'
                    | '!'
                    | '<'
                    | '>'
                    | '|'
                    | '~'
            ) {
                out.push('\\');
            }
            out.push(ch);
        }
    }
    let mut out = String::with_capacity(bio.len() + links.len() * 64);
    let mut at = 0;
    let mut sorted: Vec<&BioLink> = links
        .iter()
        .filter(|link| link.start < link.end && link.end <= bio.len())
        .filter(|link| bio.is_char_boundary(link.start) && bio.is_char_boundary(link.end))
        .collect();
    sorted.sort_by_key(|link| link.start);
    for link in sorted {
        if link.start < at {
            continue;
        }
        escape(&bio[at..link.start], &mut out);
        out.push('[');
        escape(&bio[link.start..link.end], &mut out);
        out.push_str("](");
        // A target with a space or a paren would end the link early.
        out.push_str(
            &link
                .url
                .replace(' ', "%20")
                .replace('(', "%28")
                .replace(')', "%29"),
        );
        out.push(')');
        at = link.end;
    }
    escape(&bio[at..], &mut out);
    out
}

fn fold_name(name: &str) -> String {
    providers::normalize(&rox_library::fold::fold(name))
}

/// Whole symbols rather than split credits: the filter matches a whole value,
/// so a guest on a "feat." credit would read as held and then find nothing.
fn held_index(tables: [(FilterField, &[String]); 2]) -> HashMap<String, (FilterField, String)> {
    let mut index = HashMap::new();
    for (field, strings) in tables {
        for name in strings.iter().filter(|name| !name.is_empty()) {
            index
                .entry(fold_name(name))
                .or_insert_with(|| (field, name.clone()));
        }
    }
    index
}

/// The acts an artist tag credits, in order. Semicolons, slashes, and
/// featuring joins always split. A comma splits unless the joined parts name
/// an act `is_known` vouches for, so "Earth, Wind & Fire" stays whole; the
/// longest known run wins.
///
/// An ampersand gets both readings, the whole credit and then each side,
/// since the album artists can't tell a duo from a collab. A run the comma
/// rule glued back together is exempt.
fn credits(tag: &str, is_known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for segment in tag.split([';', '/']) {
        for run in split_features(segment) {
            let parts: Vec<&str> = run
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .collect();
            let mut i = 0;
            while i < parts.len() {
                let mut end = i + 1;
                for j in (i + 2..=parts.len()).rev() {
                    if is_known(&parts[i..j].join(", ")) {
                        end = j;
                        break;
                    }
                }
                let name = parts[i..end].join(", ");
                let glued = end - i > 1;
                i = end;

                let sides: Vec<String> = if glued {
                    Vec::new()
                } else {
                    name.split(" & ")
                        .map(str::trim)
                        .filter(|side| !side.is_empty())
                        .map(String::from)
                        .collect()
                };

                out.push(name);
                if sides.len() > 1 {
                    out.extend(sides);
                }
            }
        }
    }
    out
}

fn split_features(segment: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = segment;
    loop {
        let lower = rest.to_lowercase();
        let cut = [" feat. ", " ft. ", " featuring ", " feat ", " ft "]
            .iter()
            .filter_map(|join| lower.find(join).map(|at| (at, join.len())))
            .min_by_key(|(at, _)| *at);
        match cut {
            Some((at, len)) => {
                let head = rest[..at].trim();
                if !head.is_empty() {
                    out.push(head);
                }
                rest = &rest[at + len..];
            }
            None => {
                let tail = rest.trim();
                if !tail.is_empty() {
                    out.push(tail);
                }
                return out;
            }
        }
    }
}

/// An ISO 3166 alpha-2 code as its regional-indicator pair, which the emoji
/// font draws as the flag.
fn flag(code: &str) -> Option<SharedString> {
    let code = code.trim();
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let flag: String = code
        .bytes()
        .map(|b| char::from_u32(0x1F1E6 + u32::from(b.to_ascii_uppercase() - b'A')))
        .collect::<Option<String>>()?;
    Some(flag.into())
}

/// Scaled by `base`'s own alpha, so a translucent theme's floor stays
/// translucent under the gradient.
fn scrim(base: Rgba, a: u8) -> Rgba {
    palette::alpha(base, (f32::from(a) * base.a).round() as u8)
}

fn heading(text: SharedString) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

/// None for an act marked disbanded with no end year, where "since" would be
/// wrong.
fn years_active(profile: &ArtistProfile) -> Option<SharedString> {
    let from = profile.formed?;
    match profile.ended {
        Some(to) => Some(rox_i18n::t!(
            "biography-active-between",
            from = from.to_string(),
            to = to.to_string()
        )),
        None if profile.disbanded => None,
        None => Some(rox_i18n::t!(
            "biography-active-since",
            from = from.to_string()
        )),
    }
}

fn quiet(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(tokens::SPACE_MD)
        .child(div().text_color(palette::text_faint()).child(text.into()))
}

fn loading(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_MD)
        .text_color(palette::text_faint())
        .child(Spinner::new().with_size(Size::Small))
        .child(div().child(text.into()))
}

fn tag_chip(i: usize, tag: String, query: Option<gpui::Entity<SharedQuery>>) -> gpui::AnyElement {
    let Some(query) = query else {
        return chip(tag).into_any_element();
    };
    let value = tag.clone();
    chip(tag)
        .id(("biography-tag", i))
        .cursor_pointer()
        .hover(|d| d.text_color(palette::accent()))
        .on_click(move |_, _, cx| shared_query::toggle_pick(&query, FilterField::Genre, &value, cx))
        .into_any_element()
}

fn chip(tag: String) -> Div {
    div()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .rounded_full()
        .bg(palette::bg_control())
        .text_xs()
        .text_color(palette::text_secondary())
        .child(SharedString::from(tag))
}

fn fmt_count(n: u64) -> String {
    let scaled = |v: f64, suffix: &str| {
        let text = if v >= 100.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.1}")
        };
        format!("{}{}", text.trim_end_matches(".0"), suffix)
    };
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => scaled(n as f64 / 1e3, "k"),
        _ => scaled(n as f64 / 1e6, "M"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_scale_readably() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1_500), "1.5k");
        assert_eq!(fmt_count(20_000), "20k");
        assert_eq!(fmt_count(123_456), "123k");
        assert_eq!(fmt_count(2_400_000), "2.4M");
    }

    #[test]
    fn credits_split_lists_and_features() {
        let none = |_: &str| false;
        assert_eq!(
            credits("BABYMETAL, Electric Callboy", &none),
            ["BABYMETAL", "Electric Callboy"]
        );
        assert_eq!(
            credits("Poppy feat. BABYMETAL", &none),
            ["Poppy", "BABYMETAL"]
        );
        assert_eq!(credits("A ft. B / C; D", &none), ["A", "B", "C", "D"]);
        assert_eq!(credits("  ", &none), Vec::<String>::new());
    }

    #[test]
    fn an_ampersand_reads_whole_first_and_then_each_side() {
        let none = |_: &str| false;
        assert_eq!(
            credits("Teddy Killerz & Billain", &none),
            ["Teddy Killerz & Billain", "Teddy Killerz", "Billain"]
        );
        assert_eq!(
            credits("Teddy Killerz & Billain, Foo", &none),
            ["Teddy Killerz & Billain", "Teddy Killerz", "Billain", "Foo"]
        );
        // A duo the library files albums under still gets its sides: the
        // album artists can't tell it from a collab with a record out.
        let duo = |name: &str| fold_name(name) == fold_name("Simon & Garfunkel");
        assert_eq!(
            credits("Simon & Garfunkel", &duo),
            ["Simon & Garfunkel", "Simon", "Garfunkel"]
        );
        assert_eq!(credits("Simon & ", &none), ["Simon &"]);
    }

    #[test]
    fn bio_markdown_escapes_text_and_writes_links() {
        let bio = "Formed with Other in 1993. *Not* a list:\n\n1. one";
        let links = vec![BioLink {
            start: 12,
            end: 17,
            url: "https://x/Other (band)".into(),
        }];
        assert_eq!(
            bio_markdown(bio, &links),
            "Formed with [Other](https://x/Other%20%28band%29) in 1993. \\*Not\\* a list:\n\n1. one"
        );
        assert_eq!(bio_markdown("a [b]", &[]), "a \\[b\\]");
    }

    #[test]
    fn held_names_fold_and_keep_the_library_spelling() {
        let artists = ["Madeon".to_string(), "Mat Zo & Porter Robinson".to_string()];
        let album_artists = ["MADEON".to_string(), String::new(), "Doss".to_string()];
        let held = held_index([
            (FilterField::Artist, &artists[..]),
            (FilterField::AlbumArtist, &album_artists[..]),
        ]);
        assert_eq!(
            held.get(&fold_name("madeon")),
            Some(&(FilterField::Artist, "Madeon".to_string()))
        );
        assert_eq!(
            held.get(&fold_name("Mat Zo and Porter Robinson")),
            None,
            "an ampersand and an 'and' are different names"
        );
        assert_eq!(
            held.get(&fold_name("doss")),
            Some(&(FilterField::AlbumArtist, "Doss".to_string()))
        );
        assert!(
            !held.contains_key(""),
            "an empty symbol never counts as held"
        );
        assert_eq!(held.len(), 3);
    }

    #[test]
    fn a_known_act_keeps_its_comma() {
        let known = |name: &str| fold_name(name) == fold_name("Earth, Wind & Fire");
        assert_eq!(
            credits("Earth, Wind & Fire", &known),
            ["Earth, Wind & Fire"]
        );
        assert_eq!(
            credits("Earth, Wind & Fire, Emotions", &known),
            ["Earth, Wind & Fire", "Emotions"]
        );
        assert_eq!(
            credits("Emotions feat. Earth, Wind & Fire", &known),
            ["Emotions", "Earth, Wind & Fire"]
        );
        assert_eq!(
            credits("BABYMETAL, Electric Callboy", &known),
            ["BABYMETAL", "Electric Callboy"]
        );
    }

    #[test]
    fn country_codes_become_flags() {
        assert_eq!(
            flag("JP").map(|f| f.to_string()).as_deref(),
            Some("\u{1F1EF}\u{1F1F5}")
        );
        assert_eq!(
            flag("gb").map(|f| f.to_string()).as_deref(),
            Some("\u{1F1EC}\u{1F1E7}")
        );
        assert!(flag("").is_none());
        assert!(flag("XWX").is_none());
    }

    #[test]
    fn years_line_follows_the_record() {
        let mut profile = ArtistProfile::default();
        assert!(years_active(&profile).is_none());
        profile.formed = Some(2013);
        assert!(years_active(&profile).is_some());
        profile.disbanded = true;
        assert!(years_active(&profile).is_none());
        profile.ended = Some(2020);
        assert!(years_active(&profile).is_some());
    }
}
