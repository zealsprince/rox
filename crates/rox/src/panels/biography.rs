//! The biography panel: who the current track's artist is. A wide image
//! banner up top, then the name, the listening stats, the genre tags,
//! the wiki text, and the similar names at the foot, over the artist
//! fanart dimmed into the background - everything off the artist store's
//! cached fetches (last.fm text and stats, deezer portrait, theaudiodb
//! banner and fanart), so a shown artist reads offline from then on.
//! Which track is per-view config through [`crate::source::TrackSource`],
//! the cover panel's knob, so a duplicate can watch each. The sheet
//! scrolls as one; each block has its own toggle in the panel settings,
//! so a narrow panel can pare down to just the text.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, img, linear_color_stop, linear_gradient, point, prelude::*, px, App, Context, Div,
    EventEmitter, FocusHandle, Focusable, Image, MouseButton, ObjectFit, ScrollHandle,
    SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Sizable, Size};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::artists::{self, Artist};
use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::providers;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The header band takes the image's own aspect ratio at full width, so a
/// wide banner stays a strip and nothing crops sideways. This caps how
/// tall that gets - a square portrait fallback would otherwise run as tall
/// as the panel is wide; past the cap it crops rather than dominating.
const HEADER_MAX_H: f32 = 200.;

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
    /// The image banner across the panel top: the wide artist banner when
    /// one was found, the square portrait otherwise. Named `portrait` from
    /// when that was all it showed; a saved layout keeps its setting.
    pub portrait: bool,
    /// Keep the header image at its own proportions; off crops it to fill
    /// a fixed band instead.
    pub header_aspect: bool,
    /// Let a tall header image span the full width, however tall that runs,
    /// instead of sitting capped and centered. Only bites while the
    /// proportions are kept - a cropped fill already spans the width.
    pub header_fill: bool,
    /// The artist fanart behind the text, dimmed and fading out toward the
    /// bottom so the words keep reading.
    pub background: bool,
    /// The listeners and plays row under the name.
    pub stats: bool,
    /// The genre tag chips.
    pub tags: bool,
    /// The similar artists block at the sheet's foot.
    pub similar: bool,
}

impl Default for BiographyConfig {
    fn default() -> Self {
        BiographyConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            portrait: true,
            header_aspect: true,
            header_fill: false,
            background: true,
            stats: true,
            tags: true,
            similar: true,
        }
    }
}

pub struct BiographyPanel {
    state: AppState,
    config: BiographyConfig,
    /// The shown path's artist tag, cached because the pump notifies per
    /// frame and the lookup is a database read; empty inside for an
    /// untagged file. Cleared when the catalog changes.
    artist: Option<(PathBuf, String)>,
    /// The store's answer keyed by the folded name it was asked under;
    /// None inside is a clean miss, last.fm knowing nothing by the name.
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
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
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
        // caches so the resolve and the artist tag re-read. The store's
        // answers stay - they key on the name, not the file.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.artist = None;
                cx.notify();
            },
        );
        BiographyPanel {
            state,
            config,
            artist: None,
            loaded: None,
            pending: None,
            error: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            scroll: ScrollHandle::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// The shown path's artist tag, from the cache or one database read
    /// on a miss. Empty for an untagged file or one the library does not
    /// know.
    fn artist_for(&mut self, path: &Path, cx: &App) -> String {
        if self.artist.as_ref().map(|(p, _)| p.as_path()) != Some(path) {
            let name = self
                .state
                .library
                .read(cx)
                .meta_for(path)
                .map(|meta| meta.artist)
                .unwrap_or_default();
            self.artist = Some((path.to_path_buf(), name));
        }
        self.artist
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_default()
    }

    /// Make sure the store's answer for `name` is held or on its way:
    /// run the cache-or-fetch off the UI thread and swap the result in
    /// when it lands. `force` refetches past the store's TTL, the
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
                        let old = this.loaded.take().and_then(|(_, a)| a);
                        this.loaded = Some((key, artist));
                        this.retire(old, cx);
                    }
                    Err(e) => this.error = Some((key, format!("Lookup failed: {e}").into())),
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
    /// portrait, banner, and background for the life of the process. Same as
    /// the cover and metadata panels' retire. Skips a bitmap the freshly loaded
    /// artist still shows, which a refresh of the same artist reuses.
    fn retire(&self, old: Option<Artist>, cx: &mut App) {
        let Some(old) = old else { return };
        for image in [
            old.portrait.map(|(image, _)| image),
            old.banner.map(|(image, _)| image),
            old.background,
        ]
        .into_iter()
        .flatten()
        {
            if !self.holds(&image) {
                image.remove_asset(cx);
            }
        }
    }

    /// Whether the currently loaded artist still shows the decode behind
    /// `image`, so a same-artist refresh keeps the bitmap it just reloaded.
    fn holds(&self, image: &Arc<Image>) -> bool {
        let Some((_, Some(artist))) = &self.loaded else {
            return false;
        };
        let id = image.id();
        artist.portrait.as_ref().is_some_and(|(i, _)| i.id() == id)
            || artist.banner.as_ref().is_some_and(|(i, _)| i.id() == id)
            || artist.background.as_ref().is_some_and(|i| i.id() == id)
    }

    /// Refetch the shown artist past the store's TTL, the dropdown's
    /// Refresh: a moved portrait or a grown wiki article lands without
    /// waiting out the month.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((_, name)) = self.artist.clone() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        self.ensure_loaded(&name, true, cx);
        cx.notify();
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
        // the customize window also carries. No icon: a left-side check is
        // what shows the state, and an icon would take that slot (the source
        // flyout's note), so these read like the other panels' toggle rows.
        let entity = cx.entity();
        let toggle =
            |menu: PopupMenu, label: &'static str, checked, set: fn(&mut BiographyConfig)| {
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
        let menu = toggle(menu, "Header Image", self.config.portrait, |c| {
            c.portrait = !c.portrait
        });
        let menu = toggle(menu, "Keep Aspect Ratio", self.config.header_aspect, |c| {
            c.header_aspect = !c.header_aspect
        });
        let menu = toggle(menu, "Fill Width", self.config.header_fill, |c| {
            c.header_fill = !c.header_fill
        });
        let menu = toggle(menu, "Background", self.config.background, |c| {
            c.background = !c.background
        });
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new("Refresh")
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
            .child(panel::setting_row(
                "Header Image",
                Some("The wide artist banner across the top, or the portrait when there is no banner"),
                panel::toggle(
                    self.config.portrait,
                    |this: &mut Self, on, cx| {
                        this.config.portrait = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Keep Aspect Ratio",
                Some("Show the header at its own proportions instead of cropping it to fill a band"),
                panel::toggle(
                    self.config.header_aspect,
                    |this: &mut Self, on, cx| {
                        this.config.header_aspect = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Fill Width",
                Some("Let a tall header span the full width instead of sitting capped and centered"),
                panel::toggle(
                    self.config.header_fill,
                    |this: &mut Self, on, cx| {
                        this.config.header_fill = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Background",
                Some("The artist fanart behind the text, dimmed and fading out toward the bottom"),
                panel::toggle(
                    self.config.background,
                    |this: &mut Self, on, cx| {
                        this.config.background = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Stats",
                Some("Listeners and plays on last.fm, under the name"),
                panel::toggle(
                    self.config.stats,
                    |this: &mut Self, on, cx| {
                        this.config.stats = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Tags",
                Some("The genre tags as a chip row"),
                panel::toggle(
                    self.config.tags,
                    |this: &mut Self, on, cx| {
                        this.config.tags = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Similar Artists",
                Some("Names related listening leans toward, at the foot"),
                panel::toggle(
                    self.config.similar,
                    |this: &mut Self, on, cx| {
                        this.config.similar = on;
                        cx.notify();
                    },
                    cx,
                ),
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Biography")
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

    /// The layout dump carries the panel's config; the builder registered
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
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the cover's.
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
                    let dup = cx.new(|cx| BiographyPanel::new(state, config, cx));
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

impl Render for BiographyPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl BiographyPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        // The floor reads opaque so the window backdrop (the playing
        // track's art, ADR 10) does not bleed up behind the text; the
        // panel lays its own artist background over this instead.
        let root = div().size_full().bg(palette::bg_root_opaque());
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            return root.child(quiet("No track"));
        };
        let name = self.artist_for(&path, cx);
        if name.is_empty() {
            return root.child(quiet("No artist tag"));
        }
        self.ensure_loaded(&name, false, cx);
        let key = providers::normalize(&name);
        match &self.loaded {
            Some((k, Some(artist))) if *k == key => {
                let artist = artist.clone();
                root.child(self.sheet(&artist))
            }
            Some((k, None)) if *k == key => root.child(quiet(SharedString::from(format!(
                "Nothing found for {name}"
            )))),
            _ => match &self.error {
                Some((k, error)) if *k == key => root.child(quiet(error.clone())),
                _ => root.child(loading(SharedString::from(format!("Looking up {name}")))),
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
        let band = div().w_full().flex_none().overflow_hidden();
        if !self.config.header_aspect {
            return band.h(px(HEADER_MAX_H));
        }
        let mut band = if self.config.header_fill {
            band
        } else {
            band.max_h(px(HEADER_MAX_H))
        };
        band.style().aspect_ratio = Some(ratio);
        band
    }

    /// The loaded artist as one scrolling sheet: the banner, the name,
    /// and the blocks the config keeps on.
    fn sheet(&self, artist: &Artist) -> Div {
        let info = &artist.info;
        let mut column = div().flex().flex_col().w_full();
        // The header prefers the wide banner and falls back to the square
        // portrait; the two fit knobs shape the band around it.
        let header = artist.banner.as_ref().or(artist.portrait.as_ref());
        if self.config.portrait {
            if let Some((image, ratio)) = header {
                let band = self.header_band(*ratio);
                column = column
                    .child(band.child(img(image.clone()).object_fit(ObjectFit::Cover).size_full()));
            }
        }
        let mut content = div()
            .flex()
            .flex_col()
            .w_full()
            .p(tokens::SPACE_MD)
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .text_lg()
                    .text_color(palette::text_bright())
                    .child(SharedString::from(info.name.clone())),
            );
        if self.config.stats && (info.listeners > 0 || info.playcount > 0) {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_MD)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(format!(
                        "{} listeners",
                        fmt_count(info.listeners)
                    )))
                    .child(SharedString::from(format!(
                        "{} plays",
                        fmt_count(info.playcount)
                    ))),
            );
        }
        if self.config.tags && !info.tags.is_empty() {
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(tokens::SPACE_XS)
                    .children(info.tags.iter().map(|tag| chip(tag.clone()))),
            );
        }
        if info.bio.is_empty() {
            content = content.child(
                div()
                    .text_color(palette::text_faint())
                    .child("No biography on file"),
            );
        } else {
            // Paragraphs as the stripped text separated them; single
            // breaks inside one stay soft, gpui wraps them anyway.
            content =
                content.child(
                    div()
                        .mt(tokens::SPACE_XS)
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_SM)
                        .text_color(palette::text())
                        .children(info.bio.split("\n\n").map(|paragraph| {
                            div().child(SharedString::from(paragraph.to_owned()))
                        })),
                );
        }
        if self.config.similar && !info.similar.is_empty() {
            content = content.child(
                div()
                    .mt(tokens::SPACE_XS)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child("Similar artists"),
                    )
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
                        .child("From Last.fm"),
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
        let mut root = div().size_full().relative();
        if self.config.background {
            if let Some(image) = &artist.background {
                let base = palette::bg_root_opaque();
                root = root
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(img(image.clone()).object_fit(ObjectFit::Cover).size_full()),
                    )
                    .child(div().absolute().inset_0().bg(linear_gradient(
                        0.0,
                        // Angle 0 puts 0% at the bottom: solid there, thinning
                        // to a light dim at the top.
                        linear_color_stop(base, 0.0),
                        linear_color_stop(palette::alpha(base, 0xA6), 1.0),
                    )));
            }
        }
        root.child(scroll)
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

/// One genre tag as a chip.
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
}
