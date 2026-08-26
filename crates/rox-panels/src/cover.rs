//! The cover art panel: the current track's artwork letterboxed into
//! whatever space the panel has. Which track is per-view config through
//! [`crate::source::TrackSource`] - the playing one by default, or the
//! library selection - so a duplicate can watch each. Art comes off the
//! file on a background thread through the library's art module and is
//! cached per track; a track without art shows a dim disc instead. Every
//! change of what the panel shows - blank to art, one cover to the next,
//! art to the disc stand-in - is a short cross-fade, never a pop, the same
//! move the waveform makes.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, img, prelude::*, px, radians, relative, svg, AnyElement, App, Context, Corners,
    Div, EventEmitter, FocusHandle, Focusable, Image, ImageFormat, ObjectFit, RenderImage,
    SharedString, Subscription, Transformation, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use image::Frame;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::discs::{bake_disc, DiscShape, DISC_STYLES};
use crate::panel::{
    self, align_row, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit,
};
use crate::panel_settings;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The spin speed slider's range and default, in revolutions per minute.
/// A real disc spins far too fast to watch; the default is a lazy
/// turntable pace that keeps the art readable.
const SPIN_RPM_MIN: f32 = 1.0;
const SPIN_RPM_MAX: f32 = 60.0;
const SPIN_RPM_DEFAULT: f32 = 10.0;

/// The ramp slider's ceiling and default, in seconds from rest to full
/// speed; zero snaps straight to speed.
const SPIN_RAMP_MAX: f32 = 10.0;
const SPIN_RAMP_DEFAULT: f32 = 2.0;

/// Which picture slot the panel shows. Disc is the tag's "media"
/// picture, the CD scan; every pick falls back through the front cover,
/// any embedded picture, and folder art in the art module, so a slot the
/// file doesn't carry still shows something.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtPick {
    #[default]
    Front,
    Disc,
    Back,
    Artist,
}

impl ArtPick {
    fn kind(self) -> rox_library::art::ArtKind {
        match self {
            ArtPick::Front => rox_library::art::ArtKind::Front,
            ArtPick::Disc => rox_library::art::ArtKind::Media,
            ArtPick::Back => rox_library::art::ArtKind::Back,
            ArtPick::Artist => rox_library::art::ArtKind::Artist,
        }
    }
}

/// Dress the artwork as a physical disc, whatever picture the slot
/// carries: the face of a CD under its translucent plastic, or the label
/// of a vinyl record. Off leaves the picture flat. Lives in
/// [`crate::discs`] now that the art shelf wears the same styles;
/// re-exported here because it's this panel's config vocabulary.
pub use crate::discs::DiscStyle;

/// The cover panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct CoverConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub source: TrackSource,
    /// The picture slot to show, front by default.
    #[serde(default)]
    pub art: ArtPick,
    #[serde(default)]
    pub align: Align,
    /// Stretch the art to fill the panel, ignoring its aspect ratio,
    /// instead of letterboxing it to fit.
    #[serde(default)]
    pub stretch: bool,
    /// Spin the disc while a track plays, ramping up to speed and coasting
    /// back down on pause like a real player. Applies when the panel shows
    /// a disc: the disc art slot, or any art in a disc style.
    #[serde(default)]
    pub spin: bool,
    /// Full spin speed, in revolutions per minute.
    #[serde(default = "default_spin_rpm")]
    pub spin_rpm: f32,
    /// Seconds the spin takes from rest to full speed, and back.
    #[serde(default = "default_spin_ramp")]
    pub spin_ramp: f32,
    /// The disc dress-up: off, CD, or vinyl.
    #[serde(default)]
    pub disc_style: DiscStyle,
}

fn default_spin_rpm() -> f32 {
    SPIN_RPM_DEFAULT
}

fn default_spin_ramp() -> f32 {
    SPIN_RAMP_DEFAULT
}

impl Default for CoverConfig {
    fn default() -> Self {
        CoverConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            art: ArtPick::default(),
            align: Align::default(),
            stretch: false,
            spin: false,
            spin_rpm: SPIN_RPM_DEFAULT,
            spin_ramp: SPIN_RAMP_DEFAULT,
            disc_style: DiscStyle::Off,
        }
    }
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
    /// size itself to the letterboxed fit, and the disc bake when the
    /// panel shows it as one.
    Art(Arc<Image>, f32, Option<Arc<RenderImage>>),
}

impl Slide {
    /// Same visual target; art compares by content id so a cache drop and
    /// re-read of the same bytes never fades a cover into itself, plus the
    /// bake's identity so flipping the disc shape does fade over.
    fn same(&self, other: &Slide) -> bool {
        match (self, other) {
            (Slide::Blank, Slide::Blank)
            | (Slide::Empty, Slide::Empty)
            | (Slide::Disc, Slide::Disc) => true,
            (Slide::Art(a, _, base_a), Slide::Art(b, _, base_b)) => {
                a.id() == b.id()
                    && match (base_a, base_b) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    /// The disc bake behind the slide, if it carries one.
    fn disc_base(&self) -> Option<&Arc<RenderImage>> {
        match self {
            Slide::Art(_, _, Some(base)) => Some(base),
            _ => None,
        }
    }
}

/// Loaded cover art with its aspect ratio and, when the panel shows it as
/// a disc, the masked square bake the GPU spins; None means the track has
/// no art.
type LoadedArt = Option<(Arc<Image>, f32, Option<Arc<RenderImage>>)>;

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
    /// The disc's rotation and angular velocity in radians, and the last
    /// frame's clock for the per-frame step.
    angle: f32,
    velocity: f32,
    spin_tick: Instant,
    /// The spin speed and ramp sliders' drag state, and the panel's one
    /// in-flight readout edit.
    rpm_scrub: ScrubState,
    ramp_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
    /// Retires whatever cover is still on screen when the panel is dropped
    /// (closed or its pop-out window shut). Without it a closed panel leaves
    /// its last decoded cover pinned in gpui's never-evicting asset cache.
    _retire_on_drop: Subscription,
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
        // On drop the panel is gone, so nothing is still showing: force the
        // decoded covers out of the asset cache rather than going through the
        // showing-guarded retire, and take the published content shape with
        // it.
        let panel_id = cx.entity().entity_id();
        let _retire_on_drop = cx.on_release(move |this, cx| {
            panel::shader::forget_content_shape(panel_id);
            for slide in [
                std::mem::replace(&mut this.from, Slide::Blank),
                std::mem::replace(&mut this.to, Slide::Blank),
            ] {
                if let Slide::Art(image, _, disc) = slide {
                    image.remove_asset(cx);
                    // The disc bake bypasses the asset cache, so it leaves
                    // the sprite atlases directly; dropping an already
                    // dropped bake is a no-op.
                    if let Some(disc) = disc {
                        cx.drop_image(disc, None);
                    }
                }
            }
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
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            angle: 0.0,
            velocity: 0.0,
            spin_tick: Instant::now(),
            rpm_scrub: ScrubState::default(),
            ramp_scrub: ScrubState::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
            _retire_on_drop,
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
        let kind = self.config.art.kind();
        let disc = self.disc_mode();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        rox_library::art::cover_art_of(&path, kind).and_then(|(bytes, mime)| {
                            let format = ImageFormat::from_mime_type(&mime)?;
                            // The shape off the header alone, no decode:
                            // the art layer sizes itself by it so alignment
                            // has a fitted element to place.
                            let ratio = image::ImageReader::new(std::io::Cursor::new(&bytes))
                                .with_guessed_format()
                                .ok()
                                .and_then(|reader| reader.into_dimensions().ok())
                                .map_or(1.0, |(w, h)| w as f32 / h.max(1) as f32);
                            let base = disc
                                .and_then(|shape| bake_disc(&bytes, shape))
                                .map(|disc| Arc::new(RenderImage::new(vec![Frame::new(disc)])));
                            Some((Arc::new(Image::from_bytes(format, bytes)), ratio, base))
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
        let Slide::Art(image, _, disc) = slide else {
            return;
        };
        // The disc bake goes straight into the sprite atlases, not the
        // asset cache, so it drops from them directly unless another slide
        // still shows the same bake.
        if let Some(disc) = disc {
            let showing =
                |s: &Slide| matches!(s, Slide::Art(_, _, Some(d)) if Arc::ptr_eq(d, &disc));
            if !showing(&self.from) && !showing(&self.to) {
                cx.drop_image(disc, None);
            }
        }
        let id = image.id();
        let showing = |s: &Slide| matches!(s, Slide::Art(img, ..) if img.id() == id);
        if showing(&self.from) || showing(&self.to) {
            return;
        }
        image.remove_asset(cx);
    }

    /// Point the panel at a different picture slot: drop the cached art
    /// and the load in flight so the next render fetches the new slot,
    /// fading over once it lands.
    fn set_art(&mut self, art: ArtPick, cx: &mut Context<Self>) {
        if self.config.art == art {
            return;
        }
        self.config.art = art;
        self.reload_art(cx);
    }

    /// Drop the cached art and the load in flight so the next render
    /// fetches afresh, fading over once it lands.
    fn reload_art(&mut self, cx: &mut Context<Self>) {
        self.art = None;
        self.pending = None;
        self.generation += 1;
        cx.notify();
    }

    /// Whether the loaded art gets a disc bake, and in which shape: None
    /// for the plain picture, the bare crop for a spinning disc scan, or
    /// the picked dress-up style.
    fn disc_mode(&self) -> Option<DiscShape> {
        match self.config.disc_style {
            DiscStyle::Cd => Some(DiscShape::Cd),
            DiscStyle::Vinyl => Some(DiscShape::Vinyl),
            DiscStyle::Off if self.config.spin && self.config.art == ArtPick::Disc => {
                Some(DiscShape::Crop)
            }
            DiscStyle::Off => None,
        }
    }

    /// Pick the disc dress-up, reloading the art when the bake changes.
    fn set_disc_style(&mut self, style: DiscStyle, cx: &mut Context<Self>) {
        self.edit_disc_config(|config| config.disc_style = style, cx);
    }

    /// Flip the spin: turning it off also rests the disc upright, so a
    /// motionless disc never sits at a stray angle.
    fn set_spin(&mut self, on: bool, cx: &mut Context<Self>) {
        self.edit_disc_config(|config| config.spin = on, cx);
        if !on {
            self.angle = 0.0;
            self.velocity = 0.0;
        }
    }

    /// Flip a config knob that may change the disc bake: when it does, the
    /// art reloads so the new shape fades over.
    fn edit_disc_config(&mut self, edit: impl FnOnce(&mut CoverConfig), cx: &mut Context<Self>) {
        let before = self.disc_mode();
        edit(&mut self.config);
        if self.disc_mode() != before {
            self.reload_art(cx);
        }
        cx.notify();
    }

    /// The labelled art slots, the settings row's and the flyout's one
    /// list.
    const ART_PICKS: [(&'static str, ArtPick); 4] = [
        ("Front", ArtPick::Front),
        ("Disc", ArtPick::Disc),
        ("Back", ArtPick::Back),
        ("Artist", ArtPick::Artist),
    ];

    /// The panel's own dropdown entries: the source and artwork picks,
    /// the same knobs the customize window edits.
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
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            // Follow the panel so the picked row's tick swaps live, the
            // source flyout's rule.
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, pick) in Self::ART_PICKS {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.art == pick,
                    move |this, cx| this.set_art(pick, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu("Artwork", submenu));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            // Follow the panel so the picked row's tick swaps live, the
            // source flyout's rule.
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, style) in DISC_STYLES {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.disc_style == style,
                    move |this, cx| this.set_disc_style(style, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu("Disc Style", submenu));
        let panel = cx.entity();
        menu.separator()
            .item(panel::check_row(
                "Stretch to Fill",
                Some(icons::MAXIMIZE),
                |this: &Self| this.config.stretch,
                |this, cx| {
                    this.config.stretch = !this.config.stretch;
                    cx.notify();
                },
                &panel,
            ))
            .item(panel::check_row(
                "Spin Disc",
                Some(icons::REFRESH_CW),
                |this: &Self| this.config.spin,
                |this, cx| this.set_spin(!this.config.spin, cx),
                &panel,
            ))
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
            .child(panel::setting_row(
                "Artwork",
                Some("Which picture to show; a slot the file doesn't carry falls back to the front cover".into()),
                panel::choices(
                    &Self::ART_PICKS,
                    self.config.art,
                    |this: &mut Self, art, cx| this.set_art(art, cx),
                    cx,
                ),
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
                Some("Fill the panel, ignoring the artwork aspect ratio".into()),
                panel::toggle(
                    self.config.stretch,
                    |this: &mut Self, on, cx| {
                        this.config.stretch = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Disc Style",
                Some("Dress the artwork as a CD or as a vinyl record's label".into()),
                panel::choices(
                    &DISC_STYLES,
                    self.config.disc_style,
                    |this: &mut Self, style, cx| this.set_disc_style(style, cx),
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Spin",
                Some("Rotate the disc while a track plays; applies to the disc slot or a disc style".into()),
                panel::toggle(
                    self.config.spin,
                    |this: &mut Self, on, cx| this.set_spin(on, cx),
                    cx,
                ),
            ))
            .when(self.config.spin, |page| {
                let rpm = ((self.config.spin_rpm - SPIN_RPM_MIN) / (SPIN_RPM_MAX - SPIN_RPM_MIN))
                    .clamp(0., 1.);
                let ramp = (self.config.spin_ramp / SPIN_RAMP_MAX).clamp(0., 1.);
                page.child(panel::setting_row(
                    "Spin Speed",
                    Some("Full speed, in revolutions per minute".into()),
                    panel::value_slider_edit(
                        &self.rpm_scrub,
                        &self.value_edit,
                        rpm,
                        format!("{} rpm", self.config.spin_rpm.round() as u32),
                        format!("{}", self.config.spin_rpm.round() as u32),
                        |v| (v - SPIN_RPM_MIN) / (SPIN_RPM_MAX - SPIN_RPM_MIN),
                        |this: &mut Self, fraction, cx| {
                            this.config.spin_rpm =
                                (SPIN_RPM_MIN + fraction * (SPIN_RPM_MAX - SPIN_RPM_MIN)).round();
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    "Spin Ramp",
                    Some("How long the disc takes to reach full speed, and to coast back down".into()),
                    panel::value_slider_edit(
                        &self.ramp_scrub,
                        &self.value_edit,
                        ramp,
                        format!("{:.1}s", self.config.spin_ramp),
                        format!("{:.1}", self.config.spin_ramp),
                        |v| v / SPIN_RAMP_MAX,
                        |this: &mut Self, fraction, cx| {
                            this.config.spin_ramp =
                                (fraction * SPIN_RAMP_MAX * 10.0).round() / 10.0;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
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
                CoverArtPanel::new(state, config, cx)
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

/// One slide at a weight, filling the panel. Opacity cascades to the
/// subtree, so the whole slide fades as one; where the content sits when
/// the panel is wider than it is the alignment knob. The art carries the
/// panel theme's rounding itself: gpui content masks stay rectangular,
/// so the body's rounded corners would otherwise be painted square over
/// by a cover running edge to edge.
fn layer(
    slide: &Slide,
    angle: f32,
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
        // The disc'd art: the square bake in the same letterboxed fit,
        // spun on the GPU about its center. A disc keeps its circle, so
        // the stretch and the corner rounding don't apply.
        Slide::Art(_, _, Some(disc)) => {
            let disc = disc.clone();
            let mut frame = div().w_full().max_h_full();
            frame.style().aspect_ratio = Some(1.0);
            base.child(
                frame.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| {
                            let _ = window.paint_image_transformed(
                                bounds,
                                Corners::default(),
                                disc,
                                0,
                                false,
                                Transformation::rotate(radians(angle)),
                            );
                        },
                    )
                    .size_full(),
                ),
            )
        }
        // The frame hugs the letterboxed fit instead of filling the panel -
        // full width, the height cap transferring back through the art's
        // own ratio - so the alignment above has something to place.
        // Stretch fills the panel edge to edge, dropping the aspect ratio
        // and the alignment along with it; the letterboxed fit keeps both.
        Slide::Art(image, _, None) if stretch => base.child(
            img(image.clone())
                .object_fit(ObjectFit::Fill)
                .size_full()
                .when_some(rounding, |d, radius| d.rounded(px(radius))),
        ),
        Slide::Art(image, ratio, None) => {
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
            Some(key) => {
                // Art is a property of the file, not the track: every cue
                // track of one image shares its cover, so the cache stays
                // keyed on the path and a boundary between two of them
                // reloads nothing.
                let path = key.path;
                self.ensure_art(&path, cx);
                let target = match &self.art {
                    Some((cached, art)) if *cached == path => Some(match art {
                        Some((image, ratio, base)) => {
                            Slide::Art(image.clone(), *ratio, base.clone())
                        }
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

        // Tell the panel's shader surface what shape the slide actually
        // takes in the body rect, so a frame shader can hug the picture
        // instead of guessing at a square: the art's own ratio letterboxed,
        // 1 for the square stand-ins and the disc bake, and the whole rect
        // under stretch. The settling target speaks for a fade in flight.
        let shape = match &self.to {
            Slide::Blank => 0.0,
            Slide::Empty | Slide::Disc | Slide::Art(_, _, Some(_)) => 1.0,
            Slide::Art(_, _, None) if self.config.stretch => -1.0,
            Slide::Art(_, ratio, None) => *ratio,
        };
        panel::shader::note_content_shape(cx.entity().entity_id(), shape);

        // The spin: velocity ramps toward full speed while a track plays
        // and back to rest when it stops, the angle integrating per frame.
        // Runs only with a disc bake on screen, so the plain picture never
        // pays for it.
        let has_disc = self.to.disc_base().is_some() || self.from.disc_base().is_some();
        let mut spinning = false;
        if has_disc {
            let dt = self.spin_tick.elapsed().as_secs_f32().min(0.1);
            let full = self.config.spin_rpm.max(0.0) * TAU / 60.0;
            let target = if self.config.spin && self.state.player.read(cx).is_playing() {
                full
            } else {
                0.0
            };
            if self.config.spin_ramp <= f32::EPSILON || full <= f32::EPSILON {
                self.velocity = target;
            } else {
                let step = full / self.config.spin_ramp * dt;
                self.velocity += (target - self.velocity).clamp(-step, step);
            }
            self.angle = (self.angle + self.velocity * dt).rem_euclid(TAU);
            spinning = self.velocity != 0.0 || target != 0.0;
        }
        self.spin_tick = Instant::now();

        // Frames only while a fade or the spin is actually running; a
        // settled panel costs zero.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 || spinning {
            window.request_animation_frame();
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);

        let angle = self.angle;
        let align = self.config.align;
        let rounding = self.config.chrome.theme.rounding;
        let stretch = self.config.stretch;
        // The layers hang off an in-flow inner wrapper, not the root the
        // theme pads: absolute insets resolve against the container minus
        // its border only, so on the root the frame padding would never
        // reach them.
        let inner = div().size_full().relative();
        let inner = if u >= 1.0 {
            inner.child(layer(&self.to, angle, 1.0, align, rounding, stretch))
        } else {
            // Hold the outgoing cover at full under an incoming one so a
            // same-art track change never dips toward the background, the
            // backdrop's move. A disc'd incoming has transparent surround
            // the old art would show through, and everything else coming in
            // (the stand-ins) covers nothing, so both cross-fade instead.
            let floor = if matches!(self.to, Slide::Art(_, _, None)) {
                1.0
            } else {
                1.0 - u
            };
            inner
                .child(layer(&self.from, angle, floor, align, rounding, stretch))
                .child(layer(&self.to, angle, u, align, rounding, stretch))
        };
        div()
            .size_full()
            .bg(palette::bg_root())
            .relative()
            .child(inner)
    }
}
