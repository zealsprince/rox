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
    div, img, prelude::*, px, svg, AnyElement, App, Context, EventEmitter, FocusHandle, Focusable,
    Image, ImageFormat, ObjectFit, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::palette;
use crate::panel::{self, AppState, Customizable};
use crate::panels::library::LibraryEvent;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The cover panel's per-view config: what a saved layout restores, and
/// what the customize window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CoverConfig {
    #[serde(default)]
    pub source: TrackSource,
}

/// How long a slide change takes, the waveform's reveal pace.
const FADE_SECS: f32 = 0.35;

/// One thing the panel can show. The fade runs between two of these.
#[derive(Clone)]
enum Slide {
    /// Nothing at all: what the first slide fades in from.
    Blank,
    /// The source points at no track; which message depends on the source.
    Empty(&'static str),
    /// The track has no art anywhere: the dim disc stand-in.
    Disc,
    /// A track's artwork.
    Art(Arc<Image>),
}

impl Slide {
    /// Same visual target; art compares by content id so a cache drop and
    /// re-read of the same bytes never fades a cover into itself.
    fn same(&self, other: &Slide) -> bool {
        match (self, other) {
            (Slide::Blank, Slide::Blank) | (Slide::Disc, Slide::Disc) => true,
            (Slide::Empty(a), Slide::Empty(b)) => a == b,
            (Slide::Art(a), Slide::Art(b)) => a.id() == b.id(),
            _ => false,
        }
    }
}

pub struct CoverArtPanel {
    state: AppState,
    config: CoverConfig,
    /// The loaded art keyed by the track it belongs to; None inside means
    /// the track has no art. Kept so the pump's per-frame notifies never
    /// re-read the file.
    art: Option<(PathBuf, Option<Arc<Image>>)>,
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
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the caches so both the resolve and the art re-read.
        let _library_changed =
            cx.subscribe(&state.library, |this: &mut Self, _, _: &LibraryEvent, cx| {
                this.resolved.invalidate();
                this.art = None;
                cx.notify();
            });
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
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(FADE_SECS),
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
    /// an intermediate that barely painted never flashes.
    fn retarget(&mut self, slide: Slide) {
        if self.to.same(&slide) {
            return;
        }
        if self.fade_at.elapsed().as_secs_f32() >= FADE_SECS {
            self.from = self.to.clone();
        }
        self.to = slide;
        self.fade_at = Instant::now();
    }

    /// The panel's own dropdown entries: the source pick, the same knob the
    /// customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu;
        for (label, source) in [
            ("Follow Playing", TrackSource::Playing),
            ("Follow Selection", TrackSource::Selected),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .checked(self.config.source == source)
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.source = source;
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }
}

impl Customizable for CoverArtPanel {
    fn customize(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        source::source_row(
            self.config.source,
            |this: &mut Self, source, cx| {
                this.config.source = source;
                cx.notify();
            },
            cx,
        )
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
        SharedString::from("cover art")
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
        // The config block: the panel's quick entries and the customize
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, cx);
        let menu = panel::customize_item(menu, &cx.entity());
        let menu = menu.separator();
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the transports'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(PopupMenuItem::new("Duplicate").on_click(move |_, window, cx| {
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
        }));
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

/// One slide at a weight, filling the panel. Opacity cascades to the
/// subtree, so the whole slide fades as one.
fn layer(slide: &Slide, opacity: f32) -> AnyElement {
    let base = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .opacity(opacity);
    match slide {
        Slide::Blank => base,
        Slide::Empty(message) => base.text_color(palette::text_muted()).child(*message),
        Slide::Disc => base.child(
            svg()
                .path(crate::assets::icons::DISC)
                .size(px(48.))
                .text_color(palette::text_faint()),
        ),
        Slide::Art(image) => base.child(
            img(image.clone())
                .object_fit(ObjectFit::Contain)
                .size_full(),
        ),
    }
    .into_any_element()
}

impl Render for CoverArtPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.resolved.get(self.config.source, &self.state, cx) {
            None => self.retarget(Slide::Empty(self.config.source.empty_message())),
            Some(path) => {
                self.ensure_art(&path, cx);
                match &self.art {
                    Some((cached, art)) if *cached == path => self.retarget(match art {
                        Some(image) => Slide::Art(image.clone()),
                        None => Slide::Disc,
                    }),
                    // A load is still on its way; the current slide stays up
                    // and the next one fades in when it lands.
                    _ => {}
                }
            }
        }

        // Frames only while a fade is actually running; a settled panel
        // costs zero.
        let u = (self.fade_at.elapsed().as_secs_f32() / FADE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);

        let root = div().size_full().bg(palette::bg_root()).relative();
        if u >= 1.0 {
            root.child(layer(&self.to, 1.0))
        } else {
            root.child(layer(&self.from, 1.0 - u))
                .child(layer(&self.to, u))
        }
    }
}
