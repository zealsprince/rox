//! The biography panel: who the current track's artist is. A wide image
//! banner up top with the name, the country, and the years active laid
//! over its foot, then the listening stats, the genre tags, the wiki
//! text, the top tracks, and the similar names, over the artist fanart
//! dimmed into the background. It all comes from the artist store's
//! cached fetches (Last.fm text, stats, and top tracks, deezer portrait,
//! theaudiodb banner, fanarts, and facts), so a shown artist reads
//! offline from then on. The header cycles through the wide images on a
//! timer with a crossfade, and arrows on hover step through them by
//! hand. A tag crediting several acts splits into chips, one sheet per
//! act, a click apart. A top track the
//! library holds selects on a click, the app-wide selection every other
//! panel follows, and plays on a double click; one it doesn't is inert.
//! Which track is per-view config through [`crate::source::TrackSource`],
//! the cover panel's knob, so a duplicate can watch each. The sheet
//! scrolls as one; each block has its own toggle in the panel settings,
//! so a narrow panel can pare down to just the text.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, svg, App, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, ObjectFit, Rgba,
    ScrollHandle, SharedString, Subscription, WeakEntity, Window,
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

/// The header height's default and floor, in px. With the proportions
/// kept it caps how tall the band gets, so a square portrait fallback
/// doesn't run as tall as the panel is wide; with them off it is the
/// band's height outright. Typed rather than scrubbed, since the useful
/// range runs from a strip to most of a tall panel; the ceiling only
/// keeps a stray digit from making a band taller than any screen.
const HEADER_H_DEFAULT: f32 = 200.;
const HEADER_H_MIN: f32 = 40.;
const HEADER_H_MAX: f32 = 4000.;

/// The hover arrows' size on the header, in px.
const HEADER_ARROW: f32 = 28.;

/// How far up the header the title's scrim reaches, in px.
const OVERLAY_SCRIM_H: f32 = 56.;

/// The background opacity knob's default, in percent.
const BACKGROUND_OPACITY_DEFAULT: f32 = 40.;

/// How long the crossfade between two header images runs. Longer than
/// the palette's ease: a picture swapping under a title reads better
/// slow, and nothing waits on it.
const HEADER_FADE_SECS: f32 = 0.8;

/// How often the cycle checks whether its interval has passed. A second
/// keeps a changed interval taking effect promptly without the loop
/// costing anything to speak of; the stats widget ticks the same way.
const CYCLE_TICK: Duration = Duration::from_secs(1);

/// The width of the play and queue slot at a top track row's left edge,
/// kept whether or not the row has icons so the ranks line up.
const TRACK_ACTIONS_W: f32 = 36.;

/// The choices the track count knob offers.
const TOP_TRACK_COUNTS: [usize; 3] = [3, 5, 10];

/// The choices the cycle interval knob offers, in seconds.
const CYCLE_INTERVALS: [u64; 5] = [5, 10, 20, 30, 60];

/// The biography panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Missing fields take the defaults,
/// so a layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BiographyConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: TrackSource,
    /// Which tag names the artist: the track's own artist, or the album
    /// artist for a compilation or a guest-heavy record.
    pub name_source: NameSource,
    /// The image banner across the panel top: the wide artist banner when
    /// one was found, the square portrait otherwise. Named `portrait` from
    /// when that was all it showed; a saved layout keeps its setting.
    pub portrait: bool,
    /// Keep the header image at its own proportions; off crops it to fill
    /// a fixed band instead.
    pub header_aspect: bool,
    /// Let a tall header image span the full width, however tall that runs,
    /// instead of being capped and centered. Only applies while the
    /// proportions are kept: a cropped fill already spans the width.
    pub header_fill: bool,
    /// With the proportions off, fit the whole image into the band over a
    /// blurred wash of itself instead of cropping it to fill: a banner
    /// letterboxes, a portrait pillarboxes, and nothing is cut.
    pub header_blur: bool,
    /// The band's height, or the cap on it while the proportions are
    /// kept, in px.
    pub header_height: f32,
    /// Lay the name, country, and years over the header's foot instead of
    /// under it in the sheet.
    pub header_overlay: bool,
    /// Which of theaudiodb's wide images feed the header: the logo strip
    /// (the banner) and the 16:9 fanarts. The strip is off by default,
    /// since a 1000x185 lettering band reads poorly under a title; with
    /// both off, or nothing found, the square portrait stands in.
    pub header_banner: bool,
    pub header_fanart: bool,
    /// Rotate the header through the images the two sources give, with a
    /// crossfade, every `cycle_secs`.
    pub cycle: bool,
    pub cycle_secs: u64,
    /// The artist fanart behind the text, dimmed and fading out toward the
    /// bottom so the words keep reading.
    pub background: bool,
    /// How strongly the fanart shows, in percent, before the fade.
    pub background_opacity: f32,
    /// The country and years active line under the name.
    pub profile: bool,
    /// The country as its flag; off, as its two-letter code.
    pub flag: bool,
    /// The listeners and plays row under the name.
    pub stats: bool,
    /// The genre tag chips.
    pub tags: bool,
    /// The Last.fm top tracks list after the text, `top_tracks_count`
    /// long.
    pub top_tracks: bool,
    pub top_tracks_count: usize,
    /// The similar artists block at the sheet's foot.
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

/// Which tag the sheet's artist comes from.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    Artist,
    AlbumArtist,
}

pub struct BiographyPanel {
    state: AppState,
    config: BiographyConfig,
    /// The shown path's credited artists off the chosen tag, cached
    /// because the pump notifies per frame and the lookup is a database
    /// read; empty inside for an untagged file. Cleared when the catalog
    /// changes. A tag naming several acts ("A, B", "A feat. B") splits
    /// into them, and `pick` says which one the sheet is about.
    artist: Option<(TrackKey, Vec<String>)>,
    /// Which of the credited artists the sheet shows, an index into the
    /// list above; back to the first when the track changes.
    pick: usize,
    /// Every album artist the library knows, folded, the evidence the
    /// credit splitter uses to keep "Earth, Wind & Fire" whole. Built on
    /// first use and dropped when the catalog changes.
    known_acts: Option<Arc<HashSet<String>>>,
    /// The store's result, keyed by the folded name it was asked under;
    /// None inside is a clean miss, no Last.fm entry under that name.
    loaded: Option<(String, Option<Artist>)>,
    /// The folded name a fetch is running for, so a render can tell
    /// "already fetching" from "needs a fetch".
    pending: Option<String>,
    /// The last fetch's failure keyed the same way, shown quietly in
    /// place of a sheet until the track or a refresh moves things on.
    error: Option<(String, SharedString)>,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// Discards stale fetch results when the artist changes mid-flight.
    generation: u64,
    /// Which of the header images is up, an index into the cycle list.
    header_ix: usize,
    /// The image on its way out and when the crossfade started, while one
    /// runs.
    fade: Option<(SizedImage, Instant)>,
    /// When the header last moved on, the cycle's clock.
    advanced_at: Instant,
    /// Which library track each top track resolved to, keyed by the
    /// folded artist name the list belongs to; None per row is a track
    /// the library doesn't hold. Cleared when the catalog changes.
    matches: Option<(String, Vec<Option<i64>>)>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The settings slider's scrub and readout-edit state.
    opacity_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    /// The header height field, made the first time the customize window
    /// opens, with the subscription that applies what's typed.
    height_input: Option<(Entity<InputState>, Subscription)>,
    /// The tab panel this panel is currently in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
    /// Retires the shown artist's decoded images when the panel is dropped
    /// (closed or its pop-out window shut). Without it a closed panel leaves
    /// its portrait, banner, and background pinned in gpui's never-evicting
    /// asset cache.
    _retire_on_drop: Subscription,
}

impl BiographyPanel {
    pub fn new(state: AppState, config: BiographyConfig, cx: &mut Context<Self>) -> Self {
        // The sheet turns over with the track, not as it plays, so the
        // gated observe skips the pump's per-tick repaints.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags and id -> path mappings; drop the
        // caches so the resolve, the artist tag, and the top track matches
        // re-read. The store's cached results stay: they key on the name,
        // not the file.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.artist = None;
                this.known_acts = None;
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
        // The cycle's clock: a slow tick that moves the header on once the
        // interval has passed; the loop ends with the view, the stats
        // widget's shape. Idle when the cycle is off or there is one image.
        cx.spawn(async move |view, cx| loop {
            cx.background_executor().timer(CYCLE_TICK).await;
            if view.update(cx, |this, cx| this.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        BiographyPanel {
            state,
            config,
            artist: None,
            pick: 0,
            known_acts: None,
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

    /// The shown path's credited artists off the chosen tag, from the
    /// cache or one database read on a miss, the other tag standing in
    /// when the chosen one is empty. Empty for an untagged file or one
    /// the library doesn't know.
    fn credits_for(&mut self, key: &TrackKey, cx: &App) -> Vec<String> {
        if self.artist.as_ref().map(|(k, _)| k) != Some(key) {
            let known = self.known_acts(cx);
            let names = self
                .state
                .library
                .read(cx)
                .meta_for_key(key)
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
            self.artist = Some((key.clone(), names));
            self.pick = 0;
        }
        self.artist
            .as_ref()
            .map(|(_, names)| names.clone())
            .unwrap_or_default()
    }

    /// The library's album artists, folded, from the cache or one pass
    /// over the projection's symbol table. What tells a comma inside one
    /// act's name from a comma between two acts: an album is filed under
    /// the act that made it, so a name with a comma that shows up as an
    /// album artist is one act.
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

    /// The credited artist the sheet is about: the picked one, the first
    /// when the pick has gone stale. Empty when the track names none.
    fn picked(&self) -> String {
        self.artist
            .as_ref()
            .and_then(|(_, names)| names.get(self.pick).or_else(|| names.first()))
            .cloned()
            .unwrap_or_default()
    }

    /// Make sure the store's result for `name` is loaded or on its way:
    /// run the cache-or-fetch off the UI thread and swap the result in
    /// when it arrives. `force` refetches past the store's TTL, the
    /// dropdown's refresh.
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
                        // A fresh artist starts its cycle from the top,
                        // and no fade may keep a retired image on screen.
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
                // A fresh sheet reads from the top.
                this.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop a replaced artist's decoded bitmaps from gpui's asset cache. `img`
    /// keeps every distinct decode in the process-wide asset cache and never
    /// evicts on its own, so without this every artist viewed leaks its
    /// portrait, banner, fanarts, and background for the life of the
    /// process. Same as the cover and metadata panels' retire. Skips a
    /// bitmap the freshly loaded artist still shows, which a refresh of the
    /// same artist reuses.
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

    /// Refetch the shown artist past the store's TTL, the dropdown's
    /// Refresh: a moved portrait or a grown wiki article shows up without
    /// waiting out the month.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let name = self.picked();
        if name.is_empty() {
            return;
        }
        self.ensure_loaded(&name, true, cx);
        cx.notify();
    }

    /// The images the header shows: the banner and then the fanarts, each
    /// as its source toggle allows. With the cycle off only the first
    /// counts, so a banner-only setting is the header as it was before the
    /// cycle existed. The portrait stands in when the sources give nothing.
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

    /// The cycle's tick: move the header on once the interval has passed.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if !self.config.cycle || self.advanced_at.elapsed().as_secs() < self.config.cycle_secs {
            return;
        }
        self.step(1, cx);
    }

    /// Move the header `delta` images along (wrapping either way) with a
    /// crossfade from the one up now, and restart the cycle's clock so a
    /// hand-picked image gets its full interval. Nothing to do with one
    /// image or none.
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

    /// The header height field, seeded with the config's value the first
    /// time and applying every valid number typed into it from then on.
    /// A number under the floor or over the ceiling clamps as it applies;
    /// anything that isn't a number leaves the height as it was.
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

    /// Which library track each of the artist's top tracks is, computed
    /// once per artist and kept until the catalog changes. A scan of the
    /// projection: a row whose artist or album artist folds to the shown
    /// name (the tag's spelling or Last.fm's) and whose title folds to
    /// the top track's is the match, first found wins. Folding is the
    /// search's case and accent fold plus the provider's punctuation
    /// fold, so "Don't" against "Don’t" still meets.
    fn matches_for(
        &mut self,
        key: &str,
        lastfm_name: &str,
        tracks: &[TopTrack],
        cx: &App,
    ) -> Vec<Option<i64>> {
        if let Some((k, matches)) = &self.matches {
            if k == key {
                return matches.clone();
            }
        }
        let fold = |text: &str| providers::normalize(&rox_library::fold::fold(text));
        let mut names: Vec<String> = vec![fold(lastfm_name)];
        let picked = fold(&self.picked());
        if !names.contains(&picked) {
            names.push(picked);
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
                // A row credited to several acts counts for each of them,
                // so a collaboration lands under either name.
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

    /// Publish a matched top track as the app-wide selection, the way a
    /// click in the library does, so the panels that follow it turn to
    /// the track without it starting.
    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(vec![id], source, cx));
        cx.notify();
    }

    /// Play a matched top track now, or queue it after what's queued.
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

    /// The panel's own dropdown entries: the source pick, the image
    /// toggles (the customize window's, surfaced for a quick flip), and
    /// the refresh.
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
        // A checked row that flips one bool of the config, the image toggles
        // the customize window also has. No icon: the left-side check shows
        // the state, and an icon would take that slot (the source flyout's
        // note), so these read like the other panels' toggle rows.
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
        // One toggle row per config bool, the shape every row below shares.
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
                        // The credits re-read off the other tag.
                        this.artist = None;
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
            // The height is the band's height, or the cap on a proportioned
            // band; with the proportions kept and the fill on there is no
            // cap, so the row would set nothing and hides.
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

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
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
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl BiographyPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The floor at the surface opacity like every other panel, so the
        // window backdrop (the playing track's art, ADR 10) shows through
        // as much as the theme asks. The artist background lays over it.
        let root = div().size_full().bg(palette::bg_root());
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return root.child(quiet(rox_i18n::t!("content-no-track")));
        };
        let names = self.credits_for(&key, cx);
        let name = self.picked();
        if name.is_empty() {
            return root.child(quiet(rox_i18n::t!("biography-no-artist-tag")));
        }
        self.ensure_loaded(&name, false, cx);
        let key = providers::normalize(&name);
        // With several acts credited, a chip per name over whatever the
        // sheet shows, the picked one in the accent, so the other's sheet
        // is one click away.
        let picker = (names.len() > 1).then(|| {
            let pick = self.pick.min(names.len() - 1);
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(tokens::SPACE_XS)
                .p(tokens::SPACE_SM)
                .children(names.iter().enumerate().map(|(i, credit)| {
                    let picked = i == pick;
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
                            cx.notify();
                        }))
                }))
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

    /// The frame the header image fills, shaped by the two fit knobs. The
    /// image object-fits Cover into it, so it always fills the frame and
    /// crops the overflow; the frame's own shape decides what that means:
    ///
    /// - proportions off: a fixed band, so the image crops to fill it.
    /// - fill on: full width at the image's own ratio, however tall.
    /// - neither: full width at the image's ratio but capped, so a wide
    ///   banner is a strip and a tall portrait stops at the cap, cropped.
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

    /// One header image as a layer filling the band. In the fixed band
    /// with the blurred fit on, the picture sits whole and centered over
    /// its soft companion stretched to cover; otherwise it covers and
    /// crops, which in the proportioned band is no crop at all.
    fn header_layer(&self, sized: &SizedImage) -> Div {
        let fit = !self.config.header_aspect && self.config.header_blur;
        let mut layer = div().absolute().inset_0();
        if fit {
            if let Some(soft) = &sized.soft {
                layer = layer.child(
                    div().absolute().inset_0().child(
                        img(soft.clone())
                            .overflow_hidden()
                            .object_fit(ObjectFit::Cover)
                            .size_full(),
                    ),
                );
            }
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

    /// The header: the image up now over the one on its way out while a
    /// crossfade runs, the title laid over the foot when the overlay is
    /// on, and a click to skip ahead when there is more than one image.
    /// None when the header is off or the artist has no image at all.
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
        // Frames only while a fade is running; a settled header costs
        // zero. Smoothstepped so it eases out instead of stopping dead.
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
        // With more than one image, arrows at the band's edges step through
        // them by hand; they show on hover so the picture stays clean.
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
                    // Angle 0 puts 0% at the bottom: near solid under the
                    // words, clear at the strip's top so the picture shows.
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

    /// The name with the country code beside it, and the years active
    /// under it when the profile line is on and the store has the facts.
    fn title_block(&self, artist: &Artist) -> Div {
        let profile = &artist.profile;
        let mut name = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .min_w_0();
        // The country as its flag, the two regional indicators the code
        // spells; the settings window's language picker draws its flags
        // the same way, so the glyphs are known to land. A code that
        // isn't two letters keeps the chip.
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
        if self.config.profile {
            if let Some(years) = years_active(profile) {
                block = block.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(years),
                );
            }
        }
        block
    }

    /// The loaded artist as one scrolling sheet: the header, the name,
    /// and the blocks the config keeps on.
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
        // The title sits in the sheet unless the header carries it.
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
            // A tag narrows the app-wide search on the genre filter, the
            // metadata panel's rule: only while a search box is up
            // somewhere to show the pick, so a click never narrows the
            // followers with nothing on screen saying why.
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
            // The text as markdown in a text view, which is what gives it
            // selectable, copyable text and clickable links: the wiki's
            // inline links become markdown links, everything else is
            // escaped so the article can't format itself. Keyed by the
            // artist so the view's selection state doesn't carry over.
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
            content = content.child(
                div()
                    .mt(tokens::SPACE_XS)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(heading(rox_i18n::t!("biography-similar-heading")))
                    .child(
                        div()
                            .text_color(palette::text_secondary())
                            .child(SharedString::from(info.similar.join(", "))),
                    ),
            );
        }
        // The attribution the wiki's license asks for: where the text
        // came from, as a link to the artist's page.
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

        // The fanart sits behind the scrolling sheet, fixed to the panel so
        // it fades toward the panel's own bottom rather than the content's.
        // The scrim over it is heaviest at the bottom (the text runs long)
        // and only dims the top, where the header banner covers it anyway.
        // Both stops scale with the surface opacity, so a translucent
        // theme keeps the picture showing through the words' floor.
        // A flex child rather than size_full: the body's column puts the
        // credit picker above this when a track names several acts, and
        // the sheet takes what's left.
        let mut root = div().flex_1().min_h_0().w_full().relative();
        if self.config.background {
            if let Some(image) = &artist.background {
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
                        // Angle 0 puts 0% at the bottom: solid there, thinning
                        // to a light dim at the top.
                        linear_color_stop(base, 0.0),
                        linear_color_stop(scrim(base, 0xA6), 1.0),
                    )));
            }
        }
        // The bar over the sheet's right edge, the queue's arrangement:
        // gpui's overflow scroll draws none of its own.
        root.child(scroll).child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&self.scroll)),
        )
    }

    /// The top tracks block: the heading and one row per track up to the
    /// configured count. None when the block is off or the list is empty
    /// (or not fetched yet, which an online look fills in).
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

    /// One top track: the rank, the name, and the listener count at the
    /// right. A row the library holds selects on a click (the app-wide
    /// selection, so a metadata panel on Selected turns to it), plays on
    /// a double click, and reveals a play and a queue glyph on hover; one
    /// the library doesn't hold reads faint and does nothing, so the list
    /// never promises what it can't do.
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
}

/// The wiki text as markdown: every character that markdown would read
/// as formatting escaped, the inline links written as links. Paragraph
/// breaks (blank lines) pass through, which is how the text view
/// paragraphs it.
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

/// The key a name is known under for the credit splitter: case and
/// accent folded, punctuation dropped, so "Earth, Wind & Fire" and
/// "earth wind fire" meet.
fn fold_name(name: &str) -> String {
    providers::normalize(&rox_library::fold::fold(name))
}

/// The acts an artist tag credits, in its order. Semicolons, slashes,
/// and a featuring join ("feat.", "ft.", "featuring", any case) always
/// split. A comma splits unless the parts around it, joined back, name
/// an act `is_known` vouches for: the library's album artists, so
/// "Earth, Wind & Fire" stays whole while "BABYMETAL, Electric Callboy"
/// comes apart. The longest known run wins. An ampersand never splits,
/// since "Simon & Garfunkel" is one act. Trimmed, empties dropped; a
/// plain name comes back as itself.
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
                // The longest run of parts from here that names one known
                // act, else this part alone.
                let mut end = i + 1;
                for j in (i + 2..=parts.len()).rev() {
                    if is_known(&parts[i..j].join(", ")) {
                        end = j;
                        break;
                    }
                }
                out.push(parts[i..end].join(", "));
                i = end;
            }
        }
    }
    out
}

/// One segment split on its featuring joins, the pieces trimmed and
/// empties dropped.
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

/// An ISO 3166 alpha-2 code as its flag emoji: each letter to the
/// regional indicator symbol at the same offset from A, which the emoji
/// font pairs into the flag. None for anything but two ASCII letters.
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

/// A scrim stop: `base` at `a` out of 255, scaled by the alpha the
/// surface opacity already gave it, so a translucent theme's floor
/// stays translucent under the gradient rather than snapping solid.
fn scrim(base: Rgba, a: u8) -> Rgba {
    palette::alpha(base, (f32::from(a) * base.a).round() as u8)
}

/// A block heading, the small muted line over the similar names and the
/// top tracks.
fn heading(text: SharedString) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

/// The years active line, from the record's years: since the formed year
/// for an act still going, the span for one that ended. None without a
/// formed year, or for an act marked disbanded with no year on file,
/// where "since" would be wrong and a lone year says nothing.
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

/// A quiet line where the sheet would sit, the metadata panel's move.
fn quiet(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(tokens::SPACE_MD)
        .child(div().text_color(palette::text_faint()).child(text.into()))
}

/// The same quiet line, but with a spinner over it while a lookup runs, so
/// the wait reads as work in progress rather than a stuck panel.
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

/// A genre tag as a chip that picks its value on the shared search's
/// genre filter while a search box is up (`query` is Some), and plain
/// text otherwise.
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

/// One genre tag or country code as a chip.
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

/// A listener count at chip scale: exact under a thousand, one decimal
/// of k or M above, so eight digits never crowd the stats row.
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
    fn credits_split_lists_and_features_but_not_ampersands() {
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
        assert_eq!(credits("Simon & Garfunkel", &none), ["Simon & Garfunkel"]);
        assert_eq!(credits("  ", &none), Vec::<String>::new());
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
