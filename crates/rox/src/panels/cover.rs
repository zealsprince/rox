//! The cover art panel: the current track's artwork letterboxed into
//! whatever space the panel has. Which track is per-view config through
//! [`crate::source::TrackSource`] - the playing one by default, or the
//! library selection - so a duplicate can watch each. Art comes off the
//! file on a background thread through the library's art module and is
//! cached per track; a track without art shows a dim disc instead. Every
//! change of what the panel shows - blank to art, one cover to the next,
//! art to the disc stand-in - is a short cross-fade, never a pop, the same
//! move the waveform makes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    div, img, prelude::*, px, relative, svg, AnyElement, App, Context, Div, EventEmitter,
    FocusHandle, Focusable, Image, ImageFormat, ObjectFit, SharedString, Subscription, WeakEntity,
    Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The cover panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CoverConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub source: TrackSource,
    #[serde(default)]
    pub align: Align,
    /// Stretch the art to fill the panel, ignoring its aspect ratio,
    /// instead of letterboxing it to fit.
    #[serde(default)]
    pub stretch: bool,
}

/// One thing the panel can show. The fade runs between two of these.
#[derive(Clone)]
enum Slide {
    /// Nothing at all: what the first slide fades in from.
    Blank,
    /// The source points at no track: an empty sleeve stands in.
    Empty,
    /// The track has no art anywhere: the dim disc stand-in.
    Disc,
    /// A track's artwork, with its width over height so the art layer can
    /// size itself to the letterboxed fit.
    Art(Arc<Image>, f32),
}

impl Slide {
    /// Same visual target; art compares by content id so a cache drop and
    /// re-read of the same bytes never fades a cover into itself.
    fn same(&self, other: &Slide) -> bool {
        match (self, other) {
            (Slide::Blank, Slide::Blank)
            | (Slide::Empty, Slide::Empty)
            | (Slide::Disc, Slide::Disc) => true,
            (Slide::Art(a, _), Slide::Art(b, _)) => a.id() == b.id(),
            _ => false,
        }
    }
}

/// Loaded cover art with its aspect ratio; None means the track has no art.
type LoadedArt = Option<(Arc<Image>, f32)>;

pub struct CoverArtPanel {
    state: AppState,
    config: CoverConfig,
    /// The loaded art keyed by the track it belongs to, with its aspect
    /// ratio; None inside means the track has no art. Kept so the pump's
    /// per-frame notifies never re-read the file.
    art: Option<(PathBuf, LoadedArt)>,
    /// The track a load is running for, so a render can tell "already
    /// fetching" from "needs a fetch".
    pending: Option<PathBuf>,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    /// What the panel is fading from and toward, and when the fade started.
    from: Slide,
    to: Slide,
    fade_at: Instant,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl CoverArtPanel {
    pub fn new(state: AppState, config: CoverConfig, cx: &mut Context<Self>) -> Self {
        // The cover only turns over when the playing track does; the fade
        // between them drives its own frames. Gated so the pump's per-tick
        // notify does not rebuild the panel behind a settled cover.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the caches so both the resolve and the art re-read.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.art = None;
                cx.notify();
            },
        );
        CoverArtPanel {
            state,
            config,
            art: None,
            pending: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            from: Slide::Blank,
            to: Slide::Blank,
            // Backdated so a fresh panel starts settled instead of fading
            // blank into blank.
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// Make sure the art for `path` is cached or on its way: read the file
    /// off the UI thread and swap the result in when done.
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
                            // The shape off the header alone, no decode:
                            // the art layer sizes itself by it so alignment
                            // has a fitted element to place.
                            let ratio = image::ImageReader::new(std::io::Cursor::new(&bytes))
                                .with_guessed_format()
                                .ok()
                                .and_then(|reader| reader.into_dimensions().ok())
                                .map_or(1.0, |(w, h)| w as f32 / h.max(1) as f32);
                            Some((Arc::new(Image::from_bytes(format, bytes)), ratio))
                        })
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                this.art = Some((path, loaded));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Point the panel at what it should show: the same slide stays put, a
    /// different one starts a fade from whatever was showing. A fade
    /// interrupted early keeps its original source, the waveform's rule, so
    /// an intermediate that barely painted never flashes. Whatever the swap
    /// leaves behind is retired, dropping its decoded bitmap.
    fn retarget(&mut self, slide: Slide, cx: &mut App) {
        if self.to.same(&slide) {
            return;
        }
        let abandoned = if self.fade_at.elapsed().as_secs_f32() >= tokens::EASE_SECS {
            // The fade finished: the settled target becomes the new floor,
            // the outgoing floor drops away.
            std::mem::replace(&mut self.from, self.to.clone())
        } else {
            // Mid-fade: keep the original floor, abandon the intermediate
            // that barely painted.
            self.to.clone()
        };
        self.to = slide;
        self.fade_at = Instant::now();
        self.retire(abandoned, cx);
    }

    /// Drop a retired cover's decoded bitmap from gpui's asset cache, unless
    /// the same art is still on screen. Covers reach the renderer through
    /// `img`, which keeps every distinct decode in the process-wide asset
    /// cache and never evicts on its own, so without this a long session
    /// pins one full-size bitmap per album played.
    fn retire(&self, slide: Slide, cx: &mut App) {
        let Slide::Art(image, _) = slide else {
            return;
        };
        let id = image.id();
        let showing = |s: &Slide| matches!(s, Slide::Art(img, _) if img.id() == id);
        if showing(&self.from) || showing(&self.to) {
            return;
        }
        image.remove_asset(cx);
    }

    /// The panel's own dropdown entries: the source pick, the same knob the
    /// customize window edits.
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
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new("Stretch to Fill")
                .icon(Icon::default().path(icons::MAXIMIZE))
                .checked(self.config.stretch)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.stretch = !this.config.stretch;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for CoverArtPanel {
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
        &[("Content", icons::IMAGE)]
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
                "Stretch",
                Some("Fill the panel, ignoring the artwork aspect ratio"),
                panel::toggle(
                    self.config.stretch,
                    |this: &mut Self, on, cx| {
                        this.config.stretch = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for CoverArtPanel {}

impl Focusable for CoverArtPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoverArtPanel {
    fn panel_name(&self) -> &'static str {
        "cover art"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Cover Art")
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
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
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
                    let dup = cx.new(|cx| CoverArtPanel::new(state, config, cx));
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

/// One slide at a weight, filling the panel. Opacity cascades to the
/// subtree, so the whole slide fades as one; where the content sits when
/// the panel is wider than it is the alignment knob. The art carries the
/// panel theme's rounding itself: gpui content masks stay rectangular,
/// so the body's rounded corners would otherwise be painted square over
/// by a cover running edge to edge.
fn layer(
    slide: &Slide,
    opacity: f32,
    align: Align,
    rounding: Option<f32>,
    stretch: bool,
) -> AnyElement {
    let base = justify(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .opacity(opacity),
        align,
    );
    // Only the art runs edge to edge; the stand-ins keep a margin from
    // the panel sides so an alignment never presses them into the edge.
    match slide {
        Slide::Blank => base,
        // An empty sleeve: a bare outline where a cover would sit, a faint
        // note inside. Quieter than the disc, which means a track is up but
        // carries no art. It claims the space a square cover would - full
        // width, the height cap transferring through the aspect ratio - so
        // it stays a letterboxed square and the note scales with it.
        Slide::Empty => {
            let mut sleeve = div()
                .w_full()
                .max_h_full()
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .flex()
                .items_center()
                .justify_center();
            sleeve.style().aspect_ratio = Some(1.0);
            base.p(tokens::SPACE_SM).child(
                sleeve.child(
                    svg()
                        .path(crate::assets::icons::MUSIC)
                        .size(relative(0.35))
                        .text_color(palette::text_faint()),
                ),
            )
        }
        // Same square claim as the empty sleeve: a 1x1 box takes the space a
        // cover would, so the disc centers itself no matter where the
        // alignment pushes. Without it a right align presses the icon into
        // the panel edge.
        Slide::Disc => {
            let mut frame = div()
                .w_full()
                .max_h_full()
                .flex()
                .items_center()
                .justify_center();
            frame.style().aspect_ratio = Some(1.0);
            base.p(tokens::SPACE_SM).child(
                frame.child(
                    svg()
                        .path(crate::assets::icons::DISC)
                        .size(px(48.))
                        .text_color(palette::text_faint()),
                ),
            )
        }
        // The frame hugs the letterboxed fit instead of filling the panel -
        // full width, the height cap transferring back through the art's
        // own ratio - so the alignment above has something to place.
        // Stretch fills the panel edge to edge, dropping the aspect ratio
        // and the alignment along with it; the letterboxed fit keeps both.
        Slide::Art(image, _) if stretch => base.child(
            img(image.clone())
                .object_fit(ObjectFit::Fill)
                .size_full()
                .when_some(rounding, |d, radius| d.rounded(px(radius))),
        ),
        Slide::Art(image, ratio) => {
            let mut frame = div().w_full().max_h_full();
            frame.style().aspect_ratio = Some(*ratio);
            base.child(
                frame.child(
                    img(image.clone())
                        .object_fit(ObjectFit::Contain)
                        .size_full()
                        .when_some(rounding, |d, radius| d.rounded(px(radius))),
                ),
            )
        }
    }
    .into_any_element()
}

impl Render for CoverArtPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl CoverArtPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        match self.resolved.get(self.config.source, &self.state, cx) {
            None => self.retarget(Slide::Empty, cx),
            Some(path) => {
                self.ensure_art(&path, cx);
                let target = match &self.art {
                    Some((cached, art)) if *cached == path => Some(match art {
                        Some((image, ratio)) => Slide::Art(image.clone(), *ratio),
                        None => Slide::Disc,
                    }),
                    // A load is still on its way; the current slide stays up
                    // and the next one fades in when it lands.
                    _ => None,
                };
                if let Some(target) = target {
                    self.retarget(target, cx);
                }
            }
        }

        // Frames only while a fade is actually running; a settled panel
        // costs zero.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);

        let align = self.config.align;
        let rounding = self.config.chrome.theme.rounding;
        let stretch = self.config.stretch;
        let root = div().size_full().bg(palette::bg_root()).relative();
        if u >= 1.0 {
            root.child(layer(&self.to, 1.0, align, rounding, stretch))
        } else {
            // Hold the outgoing cover at full under an incoming one so a
            // same-art track change never dips toward the background, the
            // backdrop's move. When what's coming isn't a cover (the disc
            // stand-in, the empty sleeve), fade the old one out instead.
            let floor = if matches!(self.to, Slide::Art(..)) {
                1.0
            } else {
                1.0 - u
            };
            root.child(layer(&self.from, floor, align, rounding, stretch))
                .child(layer(&self.to, u, align, rounding, stretch))
        }
    }
}
