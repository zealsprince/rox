//! The cover art panel: the current track's artwork letterboxed into
//! whatever space the panel has. Which track is per-view config through
//! [`crate::source::TrackSource`], so a duplicate can watch each. Art is read
//! off the file on a background thread and cached per track. Every change of
//! what the panel shows is a short cross-fade, never a pop.

use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Corners, Div, EventEmitter, FocusHandle, Focusable, Image,
    ImageFormat, ObjectFit, RenderImage, SharedString, Subscription, Transformation, WeakEntity,
    Window, canvas, div, img, prelude::*, px, radians, relative, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use image::Frame;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::{Origin, TrackKey};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::discs::{DISC_STYLES, DiscShape, bake_disc};
use crate::panel::{
    self, Align, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit, align_row, justify,
};
use crate::panel_settings;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// Revolutions per minute. A real disc spins far too fast to watch, so the
/// default is a lazy turntable pace.
const SPIN_RPM_MIN: f32 = 1.0;
const SPIN_RPM_MAX: f32 = 60.0;
const SPIN_RPM_DEFAULT: f32 = 10.0;

const SPIN_RAMP_MAX: f32 = 10.0;
const SPIN_RAMP_DEFAULT: f32 = 2.0;

/// Disc is the tag's "media" picture. Every pick falls back through the art
/// module, so a slot the file doesn't have still shows something.
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

/// Re-exported because it's this panel's config vocabulary.
pub use crate::discs::DiscStyle;

#[derive(Clone, Serialize, Deserialize)]
pub struct CoverConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub source: TrackSource,
    #[serde(default)]
    pub art: ArtPick,
    #[serde(default)]
    pub align: Align,
    #[serde(default)]
    pub stretch: bool,
    /// Applies when the panel shows a disc: the disc art slot, or any art in
    /// a disc style.
    #[serde(default)]
    pub spin: bool,
    #[serde(default = "default_spin_rpm")]
    pub spin_rpm: f32,
    /// Seconds from rest to full speed and back; zero snaps.
    #[serde(default = "default_spin_ramp")]
    pub spin_ramp: f32,
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

#[derive(Clone)]
enum Slide {
    Blank,
    /// The source points at no track.
    Empty,
    /// The track has no art anywhere.
    Disc,
    /// A station with nothing to show yet.
    Radio,
    /// The playing station's picture. The handle belongs to the shared art
    /// entity, so this panel must never drop its decode.
    Live(Arc<Image>, f32),
    /// Art, its width over height, and the disc bake when shown as one.
    Art(Arc<Image>, f32, Option<Arc<RenderImage>>),
}

impl Slide {
    /// Art compares by content id, so a re-read of the same bytes never fades
    /// into itself; the bake by identity, so a new disc shape does fade.
    fn same(&self, other: &Slide) -> bool {
        match (self, other) {
            (Slide::Blank, Slide::Blank)
            | (Slide::Empty, Slide::Empty)
            | (Slide::Disc, Slide::Disc)
            | (Slide::Radio, Slide::Radio) => true,
            (Slide::Live(a, _), Slide::Live(b, _)) => a.id() == b.id(),
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

    fn disc_base(&self) -> Option<&Arc<RenderImage>> {
        match self {
            Slide::Art(_, _, Some(base)) => Some(base),
            _ => None,
        }
    }
}

/// None means the track has no art.
type LoadedArt = Option<(Arc<Image>, f32, Option<Arc<RenderImage>>)>;

pub struct CoverArtPanel {
    state: AppState,
    config: CoverConfig,
    /// Keyed by path, so the pump's per-frame notifies never re-read the file.
    art: Option<(PathBuf, LoadedArt)>,
    pending: Option<PathBuf>,
    live_ratio: Option<(u64, f32)>,
    resolved: ResolvedTrack,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    from: Slide,
    to: Slide,
    fade_at: Instant,
    /// Disc rotation and angular velocity, in radians.
    angle: f32,
    velocity: f32,
    spin_tick: Instant,
    rpm_scrub: ScrubState,
    ramp_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
    /// Without it a closed panel leaves its last cover pinned in gpui's
    /// never-evicting asset cache.
    _retire_on_drop: Subscription,
}

impl CoverArtPanel {
    pub fn new(state: AppState, config: CoverConfig, cx: &mut Context<Self>) -> Self {
        // Gated so the pump's per-tick notify doesn't rebuild the panel behind
        // a settled cover.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
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
        // Nothing is showing once the panel is gone, so this skips the
        // showing-guarded retire and forces the covers out.
        let panel_id = cx.entity().entity_id();
        let _retire_on_drop = cx.on_release(move |this, cx| {
            panel::shader::forget_content_shape(panel_id);
            for slide in [
                std::mem::replace(&mut this.from, Slide::Blank),
                std::mem::replace(&mut this.to, Slide::Blank),
            ] {
                if let Slide::Art(image, _, disc) = slide {
                    image.remove_asset(cx);
                    // The bake lives in the sprite atlases, not the asset
                    // cache. A double drop is a no-op.
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
            live_ratio: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            from: Slide::Blank,
            to: Slide::Blank,
            // Backdated so a fresh panel starts settled.
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            angle: 0.0,
            velocity: 0.0,
            spin_tick: Instant::now(),
            rpm_scrub: ScrubState::default(),
            ramp_scrub: ScrubState::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
            _retire_on_drop,
        }
    }

    /// A station on air takes its cover from the shared art entity, since the
    /// song on it changes under the same key.
    fn on_air(&self, key: &TrackKey, cx: &App) -> bool {
        let Some(now) = self.state.player.read(cx).now_playing() else {
            return false;
        };

        !key.is_local() && now.live && now.key == *key
    }

    /// Cached against the picture's id so the letterbox doesn't parse the
    /// header every frame.
    fn live_ratio(&mut self, image: &Arc<Image>) -> f32 {
        if let Some((id, ratio)) = self.live_ratio
            && id == image.id()
        {
            return ratio;
        }

        let ratio = image::ImageReader::new(std::io::Cursor::new(&image.bytes))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok())
            .map_or(1.0, |(w, h)| w as f32 / h.max(1) as f32);
        self.live_ratio = Some((image.id(), ratio));
        ratio
    }

    /// A `remote` row has no file or picture slots, so it reads the one
    /// picture the thumbnail store holds, whichever slot is picked.
    fn ensure_art(&mut self, path: &Path, remote: bool, cx: &mut Context<Self>) {
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
        let thumbs = remote
            .then(|| self.state.thumbs.read(cx).store_conn())
            .flatten();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let art = match thumbs {
                            // The store holds JPEG thumbnails and nothing else.
                            Some(thumbs) => {
                                rox_services::sources::art(&thumbs, &path.to_string_lossy())
                                    .map(|bytes| (bytes, "image/jpeg".to_string()))
                            }
                            None if remote => None,
                            None => rox_library::art::cover_art_of(&path, kind),
                        };
                        art.and_then(|(bytes, mime)| {
                            let format = ImageFormat::from_mime_type(&mime)?;
                            // The shape off the header alone, no decode.
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

    /// A fade interrupted early keeps its original source, so an intermediate
    /// that barely painted never flashes. What the swap drops is retired.
    fn retarget(&mut self, slide: Slide, cx: &mut App) {
        if self.to.same(&slide) {
            return;
        }
        let abandoned = if self.fade_at.elapsed().as_secs_f32() >= tokens::EASE_SECS {
            std::mem::replace(&mut self.from, self.to.clone())
        } else {
            self.to.clone()
        };
        self.to = slide;
        self.fade_at = Instant::now();
        self.retire(abandoned, cx);
    }

    /// gpui's asset cache never evicts, so without this a long session pins
    /// one full-size bitmap per album played.
    fn retire(&self, slide: Slide, cx: &mut App) {
        let Slide::Art(image, _, disc) = slide else {
            return;
        };
        // The disc bake goes straight into the sprite atlases, not the asset
        // cache.
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

    fn set_art(&mut self, art: ArtPick, cx: &mut Context<Self>) {
        if self.config.art == art {
            return;
        }
        self.config.art = art;
        self.reload_art(cx);
    }

    fn reload_art(&mut self, cx: &mut Context<Self>) {
        self.art = None;
        self.pending = None;
        self.generation += 1;
        cx.notify();
    }

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

    fn set_disc_style(&mut self, style: DiscStyle, cx: &mut Context<Self>) {
        self.edit_disc_config(|config| config.disc_style = style, cx);
    }

    fn set_spin(&mut self, on: bool, cx: &mut Context<Self>) {
        self.edit_disc_config(|config| config.spin = on, cx);
        if !on {
            self.angle = 0.0;
            self.velocity = 0.0;
        }
    }

    fn edit_disc_config(&mut self, edit: impl FnOnce(&mut CoverConfig), cx: &mut Context<Self>) {
        let before = self.disc_mode();
        edit(&mut self.config);
        if self.disc_mode() != before {
            self.reload_art(cx);
        }
        cx.notify();
    }

    fn art_picks() -> [(SharedString, ArtPick); 4] {
        [
            (rox_i18n::t!("cover-art-front"), ArtPick::Front),
            (rox_i18n::t!("cover-art-disc"), ArtPick::Disc),
            (rox_i18n::t!("cover-art-back"), ArtPick::Back),
            (rox_i18n::t!("head-piece-artist"), ArtPick::Artist),
        ]
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
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            // Follow the panel so the picked row's tick swaps live.
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, pick) in Self::art_picks() {
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
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("cover-artwork"),
            submenu,
        ));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, style) in DISC_STYLES {
                submenu = submenu.item(panel::check_row(
                    rox_i18n::t!(label),
                    None,
                    move |this: &Self| this.config.disc_style == style,
                    move |this, cx| this.set_disc_style(style, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("cover-disc-style"),
            submenu,
        ));
        let panel = cx.entity();
        menu.separator()
            .item(panel::check_row(
                rox_i18n::t!("cover-stretch-to-fill"),
                Some(icons::MAXIMIZE),
                |this: &Self| this.config.stretch,
                |this, cx| {
                    this.config.stretch = !this.config.stretch;
                    cx.notify();
                },
                &panel,
            ))
            .item(panel::check_row(
                rox_i18n::t!("cover-spin-disc"),
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
                rox_i18n::t!("cover-artwork"),
                Some(rox_i18n::t!("cover-artwork.description")),
                panel::choices_shared(
                    &Self::art_picks(),
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
                rox_i18n::t!("cover-stretch"),
                Some(rox_i18n::t!("cover-stretch.description")),
                panel::toggle(
                    self.config.stretch,
                    |this: &mut Self, on, cx| {
                        this.config.stretch = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child({
                // DISC_STYLES holds i18n keys; choices_shared wants the text.
                let styles: Vec<_> = DISC_STYLES
                    .iter()
                    .map(|(key, style)| (rox_i18n::t!(*key), *style))
                    .collect();
                panel::setting_row(
                    rox_i18n::t!("cover-disc-style"),
                    Some(rox_i18n::t!("cover-disc-style.description")),
                    panel::choices_shared(
                        &styles,
                        self.config.disc_style,
                        |this: &mut Self, style, cx| this.set_disc_style(style, cx),
                        cx,
                    ),
                )
            })
            .child(panel::setting_row(
                rox_i18n::t!("cover-spin"),
                Some(rox_i18n::t!("cover-spin.description")),
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
                    rox_i18n::t!("cover-spin-speed"),
                    Some(rox_i18n::t!("cover-spin-speed.description")),
                    panel::value_slider_edit(
                        &self.rpm_scrub,
                        &self.value_edit,
                        rpm,
                        format!(
                            "{} rpm",
                            rox_i18n::format::format_int(self.config.spin_rpm.round() as i64)
                        ),
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
                    rox_i18n::t!("cover-spin-ramp"),
                    Some(rox_i18n::t!("cover-spin-ramp.description")),
                    panel::value_slider_edit(
                        &self.ramp_scrub,
                        &self.value_edit,
                        ramp,
                        format!(
                            "{}s",
                            rox_i18n::format::format_float(f64::from(self.config.spin_ramp), 1)
                        ),
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("cover-title"),
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

/// One slide at a weight, filling the panel. The art applies the theme's
/// rounding itself: gpui content masks stay rectangular, so a cover running
/// edge to edge would paint square over the body's rounded corners.
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
    // The stand-ins keep a margin so an alignment never presses them into the
    // panel edge.
    match slide {
        Slide::Blank => base,
        // Claims a square cover's space, so it stays a letterboxed square and
        // the note scales with it.
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
        // A 1x1 box takes a cover's space, so the disc stays centered wherever
        // the alignment pushes.
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
        Slide::Radio => {
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
                        .path(crate::assets::icons::RADIO)
                        .size(px(48.))
                        .text_color(palette::text_faint()),
                ),
            )
        }
        Slide::Live(image, _) if stretch => base.child(
            img(image.clone())
                .object_fit(ObjectFit::Fill)
                .size_full()
                .when_some(rounding, |d, radius| d.rounded(px(radius))),
        ),
        Slide::Live(image, ratio) => {
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
        // Spun on the GPU. A disc keeps its circle, so the stretch and the
        // rounding don't apply.
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
        // The frame hugs the letterboxed fit so the alignment has something
        // to place. Stretch drops the ratio and the alignment.
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
        // A focus stop, which also puts the tab group on the focus path for
        // the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl CoverArtPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        match self.resolved.get(self.config.source, &self.state, cx) {
            None => self.retarget(Slide::Empty, cx),
            Some(key) if self.on_air(&key, cx) => {
                let target = match self.state.now_art.read(cx).live_art() {
                    Some(image) => {
                        let ratio = self.live_ratio(&image);
                        Slide::Live(image, ratio)
                    }

                    None => Slide::Radio,
                };
                self.retarget(target, cx);
            }
            Some(key) => {
                // Art belongs to the file, so cue tracks of one image share
                // the path-keyed cache. A row with no file reads the
                // thumbnail store instead.
                let remote = !key.is_local();
                let stand_in = if key.origin() == Origin::Radio {
                    Slide::Radio
                } else {
                    Slide::Disc
                };
                let path = key.path;
                self.ensure_art(&path, remote, cx);
                let target = match &self.art {
                    Some((cached, art)) if *cached == path => Some(match art {
                        Some((image, ratio, base)) => {
                            Slide::Art(image.clone(), *ratio, base.clone())
                        }
                        None => stand_in,
                    }),
                    // A load is on its way; the current slide stays up.
                    _ => None,
                };
                if let Some(target) = target {
                    self.retarget(target, cx);
                }
            }
        }

        // Tell the shader surface the slide's shape so a frame shader can hug
        // the picture. During a fade it's the settling target's shape.
        let shape = match &self.to {
            Slide::Blank => 0.0,
            Slide::Empty | Slide::Disc | Slide::Radio | Slide::Art(_, _, Some(_)) => 1.0,
            Slide::Art(_, _, None) | Slide::Live(..) if self.config.stretch => -1.0,
            Slide::Art(_, ratio, None) | Slide::Live(_, ratio) => *ratio,
        };
        panel::shader::note_content_shape(cx.entity().entity_id(), shape);

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
        // The layers go in an inner wrapper: absolute insets resolve against
        // the container minus its border only, so the theme's padding on the
        // root would never apply to them.
        let inner = div().size_full().relative();
        let inner = if u >= 1.0 {
            inner.child(layer(&self.to, angle, 1.0, align, rounding, stretch))
        } else {
            // Hold outgoing art at full under incoming art so a same-art
            // change never dips toward the background. A disc bake or a
            // stand-in covers nothing, so those cross-fade.
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
