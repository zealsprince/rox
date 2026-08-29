//! The EQ widget: the equalizer as one compact readout for a transport row,
//! where the curve editor would be absurd. It answers the two questions worth
//! answering from across the room, is the EQ on and is it doing anything, and
//! a click opens the real window.
//!
//! No state of its own, because the curve hasn't got any either: it's a set of
//! process-global atomics (see [`crate::player::eq_gain`] and ADR 19). The
//! setters touch a marker global on their way past, so this reads the curve
//! fresh on every paint and uses [`crate::player::observe_eq`] to tell when
//! a paint is due.

use gpui::{
    canvas, div, fill, point, prelude::*, px, svg, AnyElement, App, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, Path, Pixels, Point, SharedString, Subscription,
    WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_playback::eq::{BANDS, FREQ_MAX, FREQ_MIN, GAIN_MAX_DB};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::player;
use crate::settings::ui as settings_ui;

/// How far off flat a band has to be to count as doing something. Under this
/// it's neither audible nor visible in a 16 pixel sparkline.
const ACTIVE_DB: f32 = 0.05;

/// The sparkline's footprint. The height matches the icon beside it so the
/// two read as one row rather than a picture with a glyph stuck to it.
const SPARK_W: f32 = 44.0;
const SPARK_H: f32 = 16.0;

/// The dB the sparkline spans either side of flat. One band's ceiling rather
/// than the EQ window's wider view: at this size a summed stack clipping the
/// top edge still reads as "a lot", which is all the widget promises.
const SPARK_DB: f32 = GAIN_MAX_DB;

/// How many points the curve is sampled at across the sparkline, about one
/// per pixel. Any finer is thrown away by the raster.
const SPARK_POINTS: usize = 44;

/// What a click on the widget does.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EqClick {
    /// Open the equalizer window, or raise the open one.
    #[default]
    Open,
    /// Flip the whole curve on and off in place, no window.
    Toggle,
    /// Nothing. A readout and no more.
    Nothing,
}

/// What the widget draws.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EqReadout {
    /// The icon alone, with the band count on its badge.
    Icon,
    /// The response curve alone, for a strip that's already all glyphs.
    Curve,
    /// Both, the icon leading.
    #[default]
    Both,
}

/// The widget's config: the shared chrome, what a click does, and how much of
/// the curve is drawn.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EqWidgetConfig {
    /// The rename, theme override, and placement locks shared by every panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub click: EqClick,
    pub readout: EqReadout,
    /// Count the bands off flat on a badge over the icon.
    pub badge: bool,
}

impl Default for EqWidgetConfig {
    fn default() -> Self {
        EqWidgetConfig {
            chrome: PanelChrome::default(),
            click: EqClick::default(),
            readout: EqReadout::default(),
            badge: true,
        }
    }
}

/// The parts of the curve the widget reads off the parameters directly: the
/// switch, and how far each band is from flat. The centers and widths aren't
/// here because nothing draws off them; the sparkline's shape comes from the
/// player's response, which accounts for the rate the filters were built for.
#[derive(Clone, Copy)]
struct EqShape {
    enabled: bool,
    gains: [f32; BANDS],
}

impl EqShape {
    fn read() -> Self {
        EqShape {
            enabled: player::eq_enabled(),
            gains: std::array::from_fn(player::eq_gain),
        }
    }

    /// How many bands are pulling the sound around.
    fn active(&self) -> usize {
        self.gains
            .iter()
            .filter(|gain| gain.abs() > ACTIVE_DB)
            .count()
    }

    /// The furthest a band is from flat, keeping its sign, so the readout
    /// says which way the biggest move goes.
    fn peak(&self) -> f32 {
        self.gains.iter().copied().fold(0.0, |worst, gain| {
            if gain.abs() > worst.abs() {
                gain
            } else {
                worst
            }
        })
    }
}

pub struct EqWidgetPanel {
    state: AppState,
    config: EqWidgetConfig,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Repaints whenever the curve moves, whichever window moved it: this
    /// widget's own toggle, the EQ window, or a copy of this in another
    /// workspace.
    _eq_changed: Subscription,
}

impl EqWidgetPanel {
    pub fn new(state: AppState, config: EqWidgetConfig, cx: &mut Context<Self>) -> Self {
        EqWidgetPanel {
            state,
            config,
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _eq_changed: player::observe_eq(cx),
        }
    }

    fn clicked(&mut self, cx: &mut Context<Self>) {
        match self.config.click {
            EqClick::Open => rox_panel_api::openers::eq_window(cx),
            EqClick::Toggle => player::set_eq_enabled(!player::eq_enabled(), cx),
            EqClick::Nothing => {}
        }
    }

    /// The cascade's response across the sparkline, in dB. Off the player
    /// because the player has the device rate the running filters were built
    /// against; the EQ window's plot does the same.
    fn curve(&self, cx: &App) -> Vec<f32> {
        let player = self.state.player.read(cx);
        let (lo, hi) = (FREQ_MIN.log10(), FREQ_MAX.log10());
        (0..SPARK_POINTS)
            .map(|i| {
                let frac = i as f32 / (SPARK_POINTS - 1) as f32;
                player.eq_response_db(10f32.powf(lo + frac * (hi - lo)))
            })
            .collect()
    }

    /// The icon, colored by what the EQ is up to: accent while it's on and
    /// shaping something, plain while it's on and flat, muted while it's off.
    /// The badge counts the bands off flat, floating off the corner so the
    /// widget's footprint never shifts with the number.
    fn glyph(&self, enabled: bool, active: usize) -> Div {
        let tint = match (enabled, active) {
            (true, 1..) => palette::accent(),
            (true, _) => palette::text(),
            (false, _) => palette::text_muted(),
        };
        // A shaped curve with the switch off still earns its badge, dimmed:
        // it's the state most worth catching, since the sound is flat while
        // the settings show otherwise.
        let badge_bg = if enabled {
            palette::accent()
        } else {
            palette::alpha(palette::text_muted(), 0x99)
        };
        div()
            .relative()
            .flex_none()
            .child(
                svg()
                    .path(icons::AUDIO_LINES)
                    .size(px(16.))
                    .text_color(tint),
            )
            .when(self.config.badge && active > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-6.))
                        .left(px(10.))
                        .px(px(4.))
                        // The parent is the 16px icon, so the count has to
                        // ignore that width or a two-digit badge wraps.
                        .whitespace_nowrap()
                        .rounded_full()
                        .bg(badge_bg)
                        .text_color(palette::text_on(badge_bg))
                        .text_size(px(9.))
                        .line_height(px(12.))
                        .child(SharedString::from(active.to_string())),
                )
            })
    }

    /// The response as a sparkline: the flat line, a wash under the curve,
    /// and a stroke along it. The EQ window's plot shrunk to a glance, same
    /// triangles, minus the grid and the handles there's no room for.
    fn spark(&self, enabled: bool, curve: Vec<f32>) -> Div {
        // Read out here rather than in the paint closure: paint has no cx,
        // and the tint has to be the one the panel was themed with.
        let zero_line = palette::alpha(palette::text_muted(), 0x44);
        let wash = palette::alpha(palette::accent(), if enabled { 0x33 } else { 0x14 });
        let stroke_color = palette::alpha(palette::accent(), if enabled { 0xff } else { 0x66 });
        let face = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if w <= 0.0 || h <= 0.0 || curve.len() < 2 {
                    return;
                }
                let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
                let at = |fx: f32, fy: f32| point(px(x0 + fx * w), px(y0 + fy * h));
                let line = |a: Point<Pixels>, b: Point<Pixels>| {
                    Bounds::from_corners(a, point(b.x.max(a.x + px(1.)), b.y.max(a.y + px(1.))))
                };
                let fy = |db: f32| (0.5 - db / (2.0 * SPARK_DB)).clamp(0.0, 1.0);
                let flat = fy(0.0);
                window.paint_quad(fill(line(at(0.0, flat), at(1.0, flat)), zero_line));

                let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
                let step = 1.0 / (curve.len() - 1) as f32;
                let mut area = Path::new(at(0.0, flat));
                let mut stroke = Path::new(at(0.0, fy(curve[0])));
                for i in 0..curve.len() - 1 {
                    let (fx0, fx1) = (i as f32 * step, (i + 1) as f32 * step);
                    let (d0, d1) = (fy(curve[i]), fy(curve[i + 1]));
                    area.push_triangle((at(fx0, d0), at(fx1, d1), at(fx1, flat)), solid);
                    area.push_triangle((at(fx0, d0), at(fx1, flat), at(fx0, flat)), solid);
                    // A path has no pen width, so the stroke is the
                    // ribbon between the curve and itself nudged down.
                    let thick = 1.25 / h;
                    stroke.push_triangle((at(fx0, d0), at(fx1, d1), at(fx1, d1 + thick)), solid);
                    stroke.push_triangle(
                        (at(fx0, d0), at(fx1, d1 + thick), at(fx0, d0 + thick)),
                        solid,
                    );
                }
                window.paint_path(area, wash);
                window.paint_path(stroke, stroke_color);
            },
        )
        .absolute()
        .inset_0();
        div()
            .relative()
            .flex_none()
            .w(px(SPARK_W))
            .h(px(SPARK_H))
            .child(face)
    }

    fn body(&self, cx: &mut Context<Self>) -> Div {
        let eq = EqShape::read();
        let active = eq.active();
        let click = self.config.click;
        let curve = matches!(self.config.readout, EqReadout::Curve | EqReadout::Both)
            .then(|| self.curve(cx));
        // Copied into the hover rather than read back through a handle: these
        // are three numbers, and the panel repaints whenever they move.
        let tooltip = EqTooltip {
            enabled: eq.enabled,
            active,
            peak: eq.peak(),
            hint: hint(click, eq.enabled),
        };
        div().size_full().bg(palette::bg_root()).child(
            div()
                .id("eq-widget")
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(tokens::SPACE_XS)
                .px(tokens::SPACE_SM)
                .size_full()
                .when(click != EqClick::Nothing, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.clicked(cx)))
                })
                .when(self.config.readout != EqReadout::Curve, |d| {
                    d.child(self.glyph(eq.enabled, active))
                })
                .when_some(curve, |d, curve| d.child(self.spark(eq.enabled, curve)))
                .tooltip(move |_window, cx| cx.new(|_| tooltip.clone()).into()),
        )
    }
}

/// What the tooltip says a click will do, if anything.
fn hint(click: EqClick, enabled: bool) -> Option<SharedString> {
    match click {
        EqClick::Open => Some(rox_i18n::t!("eq-hint-open")),
        EqClick::Toggle if enabled => Some(rox_i18n::t!("eq-hint-off")),
        EqClick::Toggle => Some(rox_i18n::t!("eq-hint-on")),
        EqClick::Nothing => None,
    }
}

/// The hover note: the switch, what the curve is doing, and where a click
/// goes. Opaque like the popup menus, since it floats over panel content with
/// no backdrop of its own.
#[derive(Clone)]
struct EqTooltip {
    enabled: bool,
    active: usize,
    peak: f32,
    hint: Option<SharedString>,
}

impl Render for EqTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let shape = if self.active == 0 {
            rox_i18n::t!("eq-shape-flat")
        } else {
            rox_i18n::t!(
                "eq-shape-active",
                count = self.active as u64,
                peak = format!("{:+.1}", self.peak)
            )
        };
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .max_w(px(280.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_color(palette::text())
            .text_xs()
            .child(if self.enabled {
                rox_i18n::t!("eq-status-on")
            } else {
                rox_i18n::t!("eq-status-off")
            })
            .child(div().text_color(palette::text_muted()).child(shape))
            .when_some(self.hint.clone(), |d, hint| {
                d.child(div().text_color(palette::text_muted()).child(hint))
            })
    }
}

impl PanelSettings for EqWidgetPanel {
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("eq-click-section"),
                Some(rox_i18n::t!("eq-click-section.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("eq-click-open"), EqClick::Open),
                        (rox_i18n::t!("eq-click-toggle"), EqClick::Toggle),
                        (rox_i18n::t!("eq-click-nothing"), EqClick::Nothing),
                    ],
                    self.config.click,
                    |this: &mut Self, click, cx| {
                        this.config.click = click;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(setting_row(
                rox_i18n::t!("eq-readout-section"),
                Some(rox_i18n::t!("eq-readout-section.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("eq-readout-icon"), EqReadout::Icon),
                        (rox_i18n::t!("eq-readout-curve"), EqReadout::Curve),
                        (rox_i18n::t!("choice-both"), EqReadout::Both),
                    ],
                    self.config.readout,
                    |this: &mut Self, readout, cx| {
                        this.config.readout = readout;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        // Nothing to pin a badge to once the icon is gone.
        if self.config.readout != EqReadout::Curve {
            rows = rows.child(setting_row(
                rox_i18n::t!("eq-band-badge"),
                Some(rox_i18n::t!("eq-band-badge.description")),
                toggle(
                    self.config.badge,
                    |this: &mut Self, on, cx| {
                        this.config.badge = on;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        }
        Some(settings_ui::section(rox_i18n::t!("eq-widget-section"), None, rows).into_any_element())
    }
}

impl EventEmitter<PanelEvent> for EqWidgetPanel {}

impl Focusable for EqWidgetPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for EqWidgetPanel {
    fn panel_name(&self) -> &'static str {
        "eq widget"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("eq-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        // What the readout in force actually needs across, raised by any
        // floor the user set. The sparkline needs a real width; the icon on
        // its own fits the dock's minimum.
        let width = match self.config.readout {
            EqReadout::Icon => f32::from(rox_dock::resizable::PANEL_MIN_SIZE),
            EqReadout::Curve => SPARK_W + 16.,
            EqReadout::Both => SPARK_W + 40.,
        };
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(width), rox_dock::resizable::PANEL_MIN_SIZE),
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
        let menu = menu
            .item(
                PopupMenuItem::new(rox_i18n::t!("eq-open"))
                    .icon(Icon::default().path(icons::AUDIO_LINES))
                    .on_click(|_, _, cx| rox_panel_api::openers::eq_window(cx)),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("eq-flatten"))
                    .icon(Icon::default().path(icons::MINUS))
                    .disabled(EqShape::read().active() == 0)
                    .on_click(|_, _, cx| player::flatten_eq(cx)),
            );
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
                EqWidgetPanel::new(state, config, cx)
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

impl Render for EqWidgetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}
