//! The metadata panel: the current track's tags laid out as a sheet -
//! title and artist up top, then the labeled fields the library carries
//! (album, genre, year, duration, codec, bitrate). Which track is per-view
//! config through [`crate::source::TrackSource`], the cover panel's knob,
//! so a duplicate can watch each. The background can carry the track's
//! cover art, cropped to fill and dimmed under a scrim so the fields keep
//! reading; art comes off the file on a background thread like the cover
//! panel's and is retired the same way when the track moves on.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, App, Context, Div, EventEmitter, FocusHandle, Focusable, Image,
    ImageFormat, ObjectFit, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::palette::PanelTheme;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::player::fmt_time;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The metadata panel's per-view config: what a saved layout restores, and
/// what the settings window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataConfig {
    /// The rename shown as the tab and title text; None shows the
    /// built-in name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub source: TrackSource,
    pub align: Align,
    /// The track's cover art behind the fields, dimmed under a scrim.
    pub cover: bool,
    /// The panel's palette override.
    #[serde(skip_serializing_if = "PanelTheme::is_empty")]
    pub theme: PanelTheme,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        MetadataConfig {
            title: None,
            source: TrackSource::default(),
            align: Align::default(),
            cover: true,
            theme: PanelTheme::default(),
        }
    }
}

/// The shown track's full projection row, owned so it outlives the borrow
/// of the library.
#[derive(Clone)]
struct Details {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    genre: String,
    year: u16,
    track_no: u16,
    duration_ms: u32,
    codec: String,
    bitrate_kbps: u16,
}

pub struct MetadataPanel {
    state: AppState,
    config: MetadataConfig,
    /// The shown path's row, or None inside for a file the library does
    /// not know. Cached because the pump notifies every frame and the row
    /// lookup scans the projection; cleared when the catalog changes.
    details: Option<(PathBuf, Option<Details>)>,
    /// The loaded background art keyed by the track it belongs to; None
    /// inside means the track has no art.
    art: Option<(PathBuf, Option<Arc<Image>>)>,
    /// The track a load is running for, so a render can tell "already
    /// fetching" from "needs a fetch".
    pending: Option<PathBuf>,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl MetadataPanel {
    pub fn new(state: AppState, config: MetadataConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the caches so the resolve, the row, and the art re-read.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, _: &LibraryEvent, cx| {
                this.resolved.invalidate();
                this.details = None;
                let old = this.art.take().and_then(|(_, art)| art);
                this.retire(old, cx);
                cx.notify();
            },
        );
        MetadataPanel {
            state,
            config,
            details: None,
            art: None,
            pending: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// The shown path's row, from the cache or one projection scan on a
    /// miss. None for a file the library does not know or while the
    /// projection is still loading.
    fn details_for(&mut self, path: &Path, cx: &App) -> Option<&Details> {
        if self.details.as_ref().map(|(p, _)| p.as_path()) != Some(path) {
            let library = self.state.library.read(cx);
            let details = library.id_for(path).and_then(|id| {
                let projection = library.projection()?;
                let row = projection.db_id.iter().position(|&db_id| db_id == id)?;
                let v = projection.resolve(row as u32);
                Some(Details {
                    title: v.title.to_owned(),
                    artist: v.artist.to_owned(),
                    album_artist: v.album_artist.to_owned(),
                    album: v.album.to_owned(),
                    genre: v.genre.to_owned(),
                    year: v.year,
                    track_no: v.track_no,
                    duration_ms: v.duration_ms,
                    codec: v.codec.to_owned(),
                    bitrate_kbps: v.bitrate_kbps,
                })
            });
            self.details = Some((path.to_path_buf(), details));
        }
        self.details.as_ref().and_then(|(_, details)| details.as_ref())
    }

    /// Make sure the background art for `path` is cached or on its way:
    /// read the file off the UI thread and swap the result in when done.
    fn ensure_art(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.art.as_ref().map(|(p, _)| p.as_path()) == Some(path)
            || self.pending.as_deref() == Some(path)
        {
            return;
        }
        self.pending = Some(path.to_path_buf());
        self.generation += 1;
        let generation = self.generation;
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        rox_library::art::cover_art(&path).and_then(|(bytes, mime)| {
                            let format = ImageFormat::from_mime_type(&mime)?;
                            Some(Arc::new(Image::from_bytes(format, bytes)))
                        })
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                let old = this.art.take().and_then(|(_, art)| art);
                this.art = Some((path, loaded));
                this.retire(old, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop a replaced background's decoded bitmap from gpui's asset cache,
    /// unless the same art is what the panel holds now. Same reason as the
    /// cover panel's retire: `img` keeps every distinct decode in the
    /// process-wide asset cache and never evicts on its own.
    fn retire(&self, art: Option<Arc<Image>>, cx: &mut App) {
        let Some(old) = art else { return };
        if let Some((_, Some(current))) = &self.art {
            if current.id() == old.id() {
                return;
            }
        }
        old.remove_asset(cx);
    }

    /// The panel's own dropdown entries: the source pick and the cover
    /// background toggle, the same knobs the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let menu = source::source_menu(
            menu,
            self.config.source,
            &cx.entity(),
            |this, source, cx| {
                this.config.source = source;
                cx.notify();
            },
        );
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new("Cover Background")
                .checked(self.config.cover)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.cover = !this.config.cover;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for MetadataPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn custom_title(&self) -> Option<&str> {
        self.config.title.as_deref()
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [&'static str] {
        &["Content"]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
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
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                "cover background",
                Some("the track's cover art behind the fields"),
                panel::toggle(
                    self.config.cover,
                    |this: &mut Self, on, cx| {
                        this.config.cover = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn theme(&self) -> PanelTheme {
        self.config.theme.clone()
    }

    fn set_theme(&mut self, theme: PanelTheme, cx: &mut Context<Self>) {
        self.config.theme = theme;
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for MetadataPanel {}

impl Focusable for MetadataPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MetadataPanel {
    fn panel_name(&self) -> &'static str {
        "metadata"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.title.as_deref(), "metadata")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.title.clone().map(SharedString::from)
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, cx);
        let menu = menu.separator();
        let menu = panel_settings::rename_item(menu, &cx.entity());
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the transports'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| MetadataPanel::new(state, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

/// One labeled field of the sheet: the tag's name dimmed in a fixed
/// column, its value truncating beside it.
fn field(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .w(px(84.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(label),
        )
        .child(div().min_w_0().truncate().child(value))
}

impl Render for MetadataPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.config.theme.clone();
        panel::themed(&theme, || self.body(cx))
    }
}

impl MetadataPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let align = self.config.align;
        let root = div().size_full().bg(palette::bg_root()).relative();

        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            // The source points at no track: a quiet line where the sheet
            // would sit.
            return root.child(
                justify(
                    div().absolute().inset_0().flex().items_center(),
                    align,
                )
                .p(tokens::SPACE_MD)
                .child(div().text_color(palette::text_faint()).child("no track")),
            );
        };

        // The background layer: the track's art cropped to fill, a scrim
        // over it so the fields keep reading over busy covers. Until the
        // load lands the plain background stands in; no fade, the sheet's
        // text swaps in the same frame anyway.
        if self.config.cover {
            self.ensure_art(&path, cx);
        }
        let backdrop = self
            .config
            .cover
            .then(|| {
                self.art
                    .as_ref()
                    .filter(|(cached, _)| *cached == path)
                    .and_then(|(_, art)| art.clone())
            })
            .flatten();
        let root = root.when_some(backdrop, |root, image| {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .child(img(image).object_fit(ObjectFit::Cover).size_full()),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(palette::alpha(palette::bg_root(), 0xB8)),
            )
        });

        // An untagged file still shows something: its file name for the
        // title, no fields.
        let details = self.details_for(&path, cx).cloned();
        let title = details
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            });

        let mut fields: Vec<(&'static str, String)> = Vec::new();
        if let Some(d) = &details {
            if !d.album.is_empty() {
                fields.push(("album", d.album.clone()));
            }
            if !d.album_artist.is_empty() && d.album_artist != d.artist {
                fields.push(("album artist", d.album_artist.clone()));
            }
            if d.track_no > 0 {
                fields.push(("track", format!("{:02}", d.track_no)));
            }
            if !d.genre.is_empty() {
                fields.push(("genre", d.genre.clone()));
            }
            if d.year > 0 {
                fields.push(("year", d.year.to_string()));
            }
            if d.duration_ms > 0 {
                fields.push(("duration", fmt_time(d.duration_ms as f64 / 1000.0)));
            }
            if !d.codec.is_empty() {
                fields.push(("codec", d.codec.clone()));
            }
            if d.bitrate_kbps > 0 {
                fields.push(("bitrate", format!("{} kbps", d.bitrate_kbps)));
            }
        }
        let artist = details
            .as_ref()
            .map(|d| d.artist.clone())
            .filter(|a| !a.is_empty());

        // The sheet: title over artist, the fields below, placed by the
        // alignment knob and centered vertically like the cover.
        let sheet = div()
            .max_w_full()
            .min_w_0()
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_lg()
                    .text_color(palette::text_bright())
                    .max_w_full()
                    .truncate()
                    .child(title),
            )
            .when_some(artist, |d, artist| {
                d.child(
                    div()
                        .text_color(palette::text_muted())
                        .max_w_full()
                        .truncate()
                        .child(artist),
                )
            })
            .when(!fields.is_empty(), |d| {
                d.child(
                    div()
                        .mt(tokens::SPACE_MD)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(fields.into_iter().map(|(label, value)| field(label, value))),
                )
            });

        root.child(
            justify(
                div().absolute().inset_0().flex().items_center(),
                align,
            )
            .child(sheet),
        )
    }
}
