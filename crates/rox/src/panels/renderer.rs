//! The Renderer panel (ADR 37, amended 2026-11-26): one renderer drawing the
//! whole panel, docked, tabbed and popped out like any other. It's a second
//! host for the canvas's render target, through the same [`Shown`] session,
//! supervisor, frame paths and grants, so a renderer can't tell the two
//! apart.
//!
//! The renderer is named the way a plugin's panel names its plugin, as
//! `plugin:<id>` in the chrome's `owner`, so a saved layout lists it in
//! `requires`. A size is fixed while a session runs: a fixed resolution is
//! drawn fitted or filled into the panel, and Match the Panel starts the
//! renderer again at the panel's own size once a resize has held still. The
//! pointer goes to the renderer except the right button, which opens the
//! panel's menu. Nothing here approves or switches anything on: a renderer
//! that isn't ready raises its enable card, and only the card's Switch On
//! approves.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, Pixels, Point, ScrollWheelEvent, SharedString, Subscription, Task,
    WeakEntity, Window, anchored, canvas, deferred, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_canvas::Projected;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_dock::{Panel, PanelEvent, PanelState, TabPanel};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings, Query};
use rox_panel_api::panel_settings;
use rox_panel_kit::ui::{self as settings_ui, PageBody, Section};
use rox_panel_kit::{choices_shared, picker, toggle};
use serde::{Deserialize, Serialize};

use crate::canvas::target::{self, CoverAsk, Fix, Key, Pointer, Readiness, Reading, Shown};
use crate::canvas::texture::WHOLE;
use crate::enable_card::EnableCard;
use crate::panels::canvas::{RENDERERS_PAGE, button_name, scroll_lines, suggests_rest};

/// The registry name.
pub const PANEL_NAME: &str = "renderer";

/// How long a panel's size has to hold before Match the Panel starts the
/// renderer again at it: past a drag's pauses, short of feeling slow.
const SETTLE: Duration = Duration::from_millis(500);

/// A new panel's resolution.
const DEFAULT_SIZE: [u32; 2] = [1280, 720];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sizing {
    /// A resolution of the panel's own, drawn fitted or filled.
    #[default]
    Fixed,
    /// The panel's size in pixels.
    Panel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// All of the picture, with the panel's background in the bands.
    #[default]
    Fit,
    /// The whole panel covered, the picture's overflow cropped.
    Fill,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RendererConfig {
    /// Its `owner` names the renderer, `plugin:<id>`, and is empty with
    /// none picked.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub sizing: Sizing,
    /// The fixed resolution, clamped to the renderers' limits where a
    /// session starts.
    pub width: u32,
    pub height: u32,
    pub scaling: Scaling,
    /// Pauses the renderer while the music is paused or stopped.
    pub rests: bool,
}

impl Default for RendererConfig {
    fn default() -> Self {
        RendererConfig {
            chrome: PanelChrome::default(),
            sizing: Sizing::Fixed,
            width: DEFAULT_SIZE[0],
            height: DEFAULT_SIZE[1],
            scaling: Scaling::Fit,
            rests: false,
        }
    }
}

/// Where the picture goes in the panel this frame: its box, panel-relative,
/// and the part of the picture that shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    /// Left, top, width, height.
    rect: [f32; 4],
    /// 0 to 1 across and down. The whole picture unless filled.
    crop: Bounds<f32>,
}

/// The picture's box for a session of `size` in a panel of `panel`.
/// Matching the panel stretches the last frame while a new size settles.
fn layout(sizing: Sizing, scaling: Scaling, size: [u32; 2], panel: [f32; 2]) -> Layout {
    let full = Layout {
        rect: [0., 0., panel[0], panel[1]],
        crop: WHOLE,
    };
    if sizing == Sizing::Panel || size[0] == 0 || size[1] == 0 || panel[1] <= 0. {
        return full;
    }

    let picture = size[0] as f32 / size[1] as f32;
    let room = panel[0] / panel[1];

    match scaling {
        Scaling::Fit if room > picture => {
            let width = panel[1] * picture;
            Layout {
                rect: [(panel[0] - width) / 2., 0., width, panel[1]],
                crop: WHOLE,
            }
        }

        Scaling::Fit => {
            let height = panel[0] / picture;
            Layout {
                rect: [0., (panel[1] - height) / 2., panel[0], height],
                crop: WHOLE,
            }
        }

        Scaling::Fill if room > picture => {
            let shown = picture / room;
            Layout {
                crop: Bounds::new(gpui::point(0., (1. - shown) / 2.), gpui::size(1., shown)),
                ..full
            }
        }

        Scaling::Fill => {
            let shown = room / picture;
            Layout {
                crop: Bounds::new(gpui::point((1. - shown) / 2., 0.), gpui::size(shown, 1.)),
                ..full
            }
        }
    }
}

/// Where a panel-relative point falls on the picture, 0 to 1 from its
/// top-left, or None over the bands around a fitted one.
fn surface_point(layout: &Layout, at: [f32; 2]) -> Option<[f32; 2]> {
    let [left, top, width, height] = layout.rect;
    if width <= 0. || height <= 0. {
        return None;
    }

    let across = [(at[0] - left) / width, (at[1] - top) / height];
    if !(0. ..=1.).contains(&across[0]) || !(0. ..=1.).contains(&across[1]) {
        return None;
    }

    let crop = layout.crop;
    Some([
        crop.origin.x + across[0] * crop.size.width,
        crop.origin.y + across[1] * crop.size.height,
    ])
}

/// The session size Match the Panel asks for: the panel in device pixels,
/// held to what a renderer can draw and rounded down to even.
fn matched_size(panel: [f32; 2], scale: f32) -> [u32; 2] {
    let side = |logical: f32, range: std::ops::RangeInclusive<u32>| {
        let pixels = (logical * scale).round().max(0.) as u32;
        (pixels - pixels % 2).clamp(*range.start(), *range.end())
    };

    [
        side(panel[0], rox_services::renderers::WIDTHS),
        side(panel[1], rox_services::renderers::HEIGHTS),
    ]
}

/// The width and height fields, each with its subscription, and the
/// settings window they belong to.
type SizeInputs = (AnyWindowHandle, [(Entity<InputState>, Subscription); 2]);

/// What the panel has raised over its window.
enum Prompt {
    /// The renderer isn't switched on: its card, then the signals offer.
    Enable {
        card: Box<EnableCard>,
        focus: FocusHandle,
    },
    /// What the renderer follows that the pool lacks, as a canvas asks on
    /// placing a target.
    Signals {
        name: String,
        words: Vec<String>,
        focus: FocusHandle,
    },
}

impl Prompt {
    fn focus(&self) -> &FocusHandle {
        match self {
            Prompt::Enable { focus, .. } | Prompt::Signals { focus, .. } => focus,
        }
    }
}

pub struct RendererPanel {
    state: AppState,
    config: RendererConfig,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The window it last drew in, for a card asked for from its settings.
    window: Option<AnyWindowHandle>,
    shown: Option<Shown>,
    reading: Reading,
    /// The panel's box from the last paint, in window coordinates.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The picture's place from the last render, for the pointer.
    layout: Layout,
    /// Match the Panel's session size, once there is one.
    matched: Option<[u32; 2]>,
    /// A new panel size and when it was first seen, until it holds.
    settling: Option<([u32; 2], Instant)>,
    /// A button held down over the window: a drag, which never restarts it.
    pressed: Rc<Cell<bool>>,
    _settle: Option<Task<()>>,
    /// The pointer is over the picture.
    over: bool,
    menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    prompt: Option<Prompt>,
    /// The fixed resolution's typed fields and the settings window they
    /// were made in: another settings window makes its own.
    size_inputs: Option<SizeInputs>,
    _player: Subscription,
}

impl RendererPanel {
    pub fn new(state: AppState, config: RendererConfig, cx: &mut Context<Self>) -> Self {
        // The cover follows the playing track between frames, and playback
        // going on wakes a renderer resting for the music, which draws
        // nothing new while it rests.
        let mut was_running = None;
        let player = cx.observe(&state.player, move |this: &mut Self, player, cx| {
            if let Some(ask) = playing_cover(&this.state, cx)
                && this.reading.follow_cover(&ask)
                && let Some(target) = this.shown.as_ref().and_then(Shown::target)
            {
                target.cover(&ask);
            }

            let player = player.read(cx);
            let running =
                crate::canvas::routes::playback_running(player.is_playing(), player.queue_ended());
            if was_running.replace(running) != Some(running) && this.config.rests {
                cx.notify();
            }
        });

        RendererPanel {
            state,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
            window: None,
            shown: None,
            reading: Reading::default(),
            bounds: Rc::default(),
            layout: layout(Sizing::Panel, Scaling::Fit, [0, 0], [0., 0.]),
            matched: None,
            settling: None,
            pressed: Rc::default(),
            _settle: None,
            over: false,
            menu: None,
            prompt: None,
            size_inputs: None,
            _player: player,
        }
    }

    fn renderer(&self) -> Option<&str> {
        target::renderer_id(&self.config.chrome.owner)
    }

    /// The renderer's name as the lists show it, for the tab.
    fn renderer_name(&self) -> Option<String> {
        let id = self.renderer()?;
        let named = rox_services::renderers::all()
            .into_iter()
            .find(|renderer| renderer.id == id)
            .map(|renderer| target::renderer_label(&renderer));

        Some(named.unwrap_or_else(|| id.to_string()))
    }

    /// Picks the renderer `owner` names, whose rest setting starts from its
    /// suggestion, as a new canvas target's does. Answers whether it needs
    /// its card first.
    fn choose(&mut self, owner: String, cx: &mut Context<Self>) -> bool {
        let off = target::renderer_id(&owner).map(Readiness::of) == Some(Readiness::Off);
        if owner != self.config.chrome.owner {
            self.config.rests = suggests_rest(&owner);
            self.config.chrome.owner = owner;
            panel::refresh_tab_panel(&self.tab_panel, cx);
        }

        cx.notify();
        off
    }

    /// A pick from the panel's own menus: one that isn't switched on raises
    /// its card here, and one that is offers what it follows.
    fn pick(&mut self, owner: String, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;

        match self.choose(owner, cx) {
            true => self.ask_enable(window, cx),
            false => self.offer_signals(window, cx),
        }
    }

    /// The card for this panel's renderer, over this window. One with no
    /// card to show, as when it doesn't load, opens the Renderers page.
    fn ask_enable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.renderer().and_then(EnableCard::for_renderer) else {
            panel_settings::open_app_page(RENDERERS_PAGE, window, cx);
            return;
        };

        let focus = cx.focus_handle();
        focus.focus(window);
        self.prompt = Some(Prompt::Enable {
            card: Box::new(card),
            focus,
        });
        cx.notify();
    }

    /// [`ask_enable`](Self::ask_enable) from the settings page, in the window
    /// the panel draws in, which comes forward with it.
    fn ask_enable_later(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.window else {
            return;
        };

        let me = cx.entity().downgrade();
        let app: &mut App = cx;
        app.defer(move |cx| {
            handle
                .update(cx, |_, window, cx| {
                    window.activate_window();
                    me.update(cx, |this, cx| this.ask_enable(window, cx)).ok();
                })
                .ok();
        });
    }

    /// Switch On: the card's approval, then what it follows.
    fn confirm_enable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Prompt::Enable { card, .. }) = self.prompt.take() else {
            return;
        };

        card.accept(cx);
        self.offer_signals(window, cx);
        cx.notify();
    }

    /// The wanted signals the pool lacks, for a renderer approved as it
    /// stands. Nothing goes in without Add.
    fn offer_signals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let missing = self.missing_signals();
        if missing.is_empty() {
            return;
        }

        let name = self.renderer_name().unwrap_or_default();
        let words = target::words_for(&missing, &self.state.signals.pool());
        let focus = cx.focus_handle();
        focus.focus(window);
        self.prompt = Some(Prompt::Signals { name, words, focus });
        cx.notify();
    }

    fn missing_signals(&self) -> Vec<rox_viz::signal::Signal> {
        let Some(id) = self.renderer() else {
            return Vec::new();
        };

        rox_services::renderers::all()
            .into_iter()
            .find(|renderer| renderer.id == id && renderer.approved)
            .map(|renderer| {
                rox_services::renderers::signals_to_add(
                    &renderer.signals,
                    &self.state.signals.pool(),
                )
            })
            .unwrap_or_default()
    }

    fn add_signals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let adding = self.missing_signals();
        self.close_prompt(window, cx);
        if adding.is_empty() {
            return;
        }

        let hub = self.state.signals.clone();
        hub.edit(|pool| {
            crate::objects::merge_signals(pool, &adding);
        });
        rox_panel_api::signal_ui::persist_pool_soon(&hub, cx);
        log::info!("renderer panel: added {} signals it asks for", adding.len());
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        window.focus(&self.focus);
        cx.notify();
    }

    /// The renderer list as a menu: every installed one, marked when it
    /// isn't ready, the current one ticked.
    fn renderer_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let choices = target::renderer_choices(&rox_services::renderers::all());
        let current = self.config.chrome.owner.clone();
        let me = cx.entity().downgrade();

        PopupMenu::build(window, cx, move |mut menu, _, _| {
            if choices.is_empty() {
                menu = menu.item(PopupMenuItem::label(rox_i18n::t!(
                    "canvas-target-panel-none"
                )));
            }

            for choice in &choices {
                let (me, picked) = (me.clone(), choice.owner.clone());
                menu = menu.item(
                    PopupMenuItem::new(choice.shown())
                        .checked(choice.owner == current)
                        .on_click(move |_, window, cx| {
                            me.update(cx, |this, cx| this.pick(picked.clone(), window, cx))
                                .ok();
                        }),
                );
            }
            menu
        })
    }

    fn open_picker(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let menu = self.renderer_menu(window, cx);
        menu.focus_handle(cx).focus(window);

        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu = None;
            cx.notify();
        });
        self.menu = Some((at, menu, dismissed));
        cx.notify();
    }

    /// The session's size this frame: the fixed one, or the panel's once it
    /// has held still for [`SETTLE`] with no button down.
    fn session_size(&mut self, window: &Window, cx: &mut Context<Self>) -> [u32; 2] {
        if self.config.sizing == Sizing::Fixed {
            self.settling = None;
            self.matched = None;
            return [self.config.width, self.config.height];
        }

        let bounds = self.bounds.get();
        let panel = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let want = matched_size(panel, window.scale_factor());

        let Some(current) = self.matched else {
            self.matched = Some(want);
            return want;
        };
        if want == current {
            self.settling = None;
            return current;
        }

        let now = Instant::now();
        let since = match self.settling {
            Some((size, since)) if size == want => since,
            _ => {
                self.settling = Some((want, now));
                now
            }
        };

        let held = now.duration_since(since);
        if held >= SETTLE && !self.pressed.get() {
            self.settling = None;
            self.matched = Some(want);
            return want;
        }

        // Look again once it may have held, or the drag has ended.
        let wait = SETTLE.saturating_sub(held) + Duration::from_millis(20);
        self._settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        }));
        current
    }

    /// This frame's reading for the renderer, as a canvas reads it for its
    /// targets: the pool, the signals while music plays and a moment after,
    /// the colours and the cover.
    fn read(&mut self, cx: &mut Context<Self>) {
        if let Some(ask) = playing_cover(&self.state, cx)
            && self.reading.follow_cover(&ask)
            && let Some(target) = self.shown.as_ref().and_then(Shown::target)
        {
            target.cover(&ask);
        }

        let art = palette::art_theming()
            .then(|| palette::seed(self.state.player.entity_id()))
            .flatten();
        // The panel paints the theme's background around the picture, solid
        // only while surfaces are opaque.
        let stage =
            (palette::app_surface_opacity() >= 1.0).then(|| palette::theme_target().bg_root);
        let colours = target::palette_line(&palette::resolved(), art.as_ref(), stage);

        let player = self.state.player.read(cx);
        let running =
            crate::canvas::routes::playback_running(player.is_playing(), player.queue_ended());
        let time = || player.now_playing().map_or(0., |now| now.position_secs);
        self.reading
            .read(&self.state.signals, running, time, colours);
    }

    /// The picture, with the session started, or moved to a new key first.
    fn picture(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let owner = self.config.chrome.owner.clone();
        if target::renderer_id(&owner).is_none() {
            if let Some(old) = self.shown.take() {
                old.release(window, cx);
            }
            return None;
        }

        let [width, height] = self.session_size(window, cx);
        let key = Key {
            owner,
            width,
            height,
        };
        if self.shown.as_ref().is_some_and(|shown| *shown.key() != key)
            && let Some(old) = self.shown.take()
        {
            old.release(window, cx);
        }

        let bounds = self.bounds.get();
        let panel = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        if panel[0] <= 0. || panel[1] <= 0. {
            return None;
        }

        self.layout = layout(
            self.config.sizing,
            self.config.scaling,
            [key.width, key.height],
            panel,
        );
        let [left, top, w, h] = self.layout.rect;
        let face = Projected {
            corners: [
                [left, top],
                [left + w, top],
                [left + w, top + h],
                [left, top + h],
            ],
            depth: 0.,
            scale: 1.,
            opacity: 1.,
        };
        let crop = (self.layout.crop != WHOLE).then_some(self.layout.crop);
        let rest = self.config.rests && !self.reading.playing();

        let shown = self.shown.get_or_insert_with(|| Shown::new(key));
        let (view, _) = shown.paint(face, crop, rest, self.reading.feed(), cx);
        view
    }

    fn status(&self) -> Option<target::Status> {
        self.renderer()?;

        Some(
            self.shown
                .as_ref()
                .and_then(Shown::target)
                .map_or(target::Status::Paused, target::Target::status),
        )
    }

    /// Sends the renderer a pointer event at a panel-relative point, and a
    /// leave once the pointer leaves the picture.
    fn point(&mut self, at: [f32; 2], pointer: impl FnOnce([f32; 2]) -> Pointer) {
        let Some(target) = self.shown.as_ref().and_then(Shown::target) else {
            return;
        };

        match surface_point(&self.layout, at) {
            Some(spot) => {
                self.over = true;
                target.point(pointer(spot));
            }
            None if self.over => {
                self.over = false;
                target.point(Pointer::Leave);
            }
            None => {}
        }
    }

    fn leave(&mut self) {
        if std::mem::take(&mut self.over)
            && let Some(target) = self.shown.as_ref().and_then(Shown::target)
        {
            target.point(Pointer::Leave);
        }
    }

    fn local(&self, position: Point<Pixels>) -> [f32; 2] {
        let origin = self.bounds.get().origin;
        [
            f32::from(position.x - origin.x),
            f32::from(position.y - origin.y),
        ]
    }

    /// The quiet line in place of a picture, with the button that fixes it.
    fn empty_state(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.status();

        let (words, button): (SharedString, Option<AnyElement>) = match &status {
            None if rox_services::renderers::all().is_empty() => (
                rox_i18n::t!("canvas-target-panel-none"),
                Some(target::fix_button(Fix::Settings, Self::fix, cx)),
            ),
            None => (
                rox_i18n::t!("canvas-target-panel-empty"),
                Some(
                    settings_ui::small_button(
                        rox_i18n::t!("canvas-target-panel-pick"),
                        icons::MONITOR_PLAY,
                        false,
                        cx.listener(|this, _, window, cx| {
                            let at = window.mouse_position();
                            this.open_picker(at, window, cx)
                        }),
                    )
                    .keyed("renderer-panel-pick")
                    .into_any_element(),
                ),
            ),
            Some(status @ (target::Status::Stopped(_) | target::Status::Starting(Some(_)))) => (
                target::status_words(status),
                target::status_fix(status).map(|fix| target::fix_button(fix, Self::fix, cx)),
            ),
            Some(_) => return None,
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(tokens::SPACE_SM)
                .p(tokens::SPACE_MD)
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(palette::text_muted())
                        .child(words),
                )
                .children(button)
                .into_any_element(),
        )
    }

    fn fix(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            Fix::SwitchOn => self.ask_enable(window, cx),
            Fix::Settings => panel_settings::open_app_page(RENDERERS_PAGE, window, cx),
            Fix::StartAgain => {
                if let Some(target) = self.shown.as_ref().and_then(Shown::target) {
                    target.start_again();
                }
                cx.notify();
            }
        }
    }

    fn menu_layer(&self, window: &Window) -> Option<AnyElement> {
        let (position, menu, _) = self.menu.as_ref()?;
        let size = window.bounds().size;

        Some(
            deferred(
                anchored().child(
                    div().w(size.width).h(size.height).occlude().child(
                        anchored()
                            .position(*position)
                            .snap_to_window_with_margin(px(8.))
                            .child(menu.clone()),
                    ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn prompt_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.prompt.as_ref()?;
        let cancel = settings_ui::dialog_button(
            rox_i18n::t!("workspace-dialog-cancel"),
            false,
            cx.listener(|this, _, window, cx| this.close_prompt(window, cx)),
        );

        let (body, buttons, width) = match prompt {
            Prompt::Enable { card, .. } => {
                let switch_on = settings_ui::dialog_button(
                    rox_i18n::t!("settings-plugins-switch-on"),
                    true,
                    cx.listener(|this, _, window, cx| this.confirm_enable(window, cx)),
                );
                (card.body(), vec![cancel, switch_on], 380.)
            }

            Prompt::Signals { name, words, .. } => {
                let line = |text: SharedString| {
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(text)
                };
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .child(div().child(rox_i18n::t!(
                        "canvas-target-signals-title",
                        name = name.as_str()
                    )))
                    .child(line(rox_i18n::t!("canvas-target-signals-body")))
                    .children(
                        words
                            .iter()
                            .map(|word| line(SharedString::from(word.clone()))),
                    );
                let not_now = settings_ui::dialog_button(
                    rox_i18n::t!("canvas-target-signals-not-now"),
                    false,
                    cx.listener(|this, _, window, cx| this.close_prompt(window, cx)),
                );
                let add = settings_ui::dialog_button(
                    rox_i18n::t!("canvas-target-signals-confirm"),
                    true,
                    cx.listener(|this, _, window, cx| this.add_signals(window, cx)),
                );
                (body, vec![not_now, add], 360.)
            }
        };

        let keys = cx.listener(|this: &mut Self, event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                this.close_prompt(window, cx);
                cx.stop_propagation();
            }
        });

        Some(crate::canvas::overlay::dialog_layer(
            "renderer-panel-dialog",
            Some(prompt.focus()),
            body,
            buttons,
            width,
            window.bounds().size,
            keys,
        ))
    }

    /// The fixed resolution's two fields, made with the settings window in
    /// hand. A preset drops them, so the next page shows its numbers.
    fn size_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [Entity<InputState>; 2] {
        if let Some((made_in, inputs)) = &self.size_inputs
            && *made_in == window.window_handle()
        {
            return [inputs[0].0.clone(), inputs[1].0.clone()];
        }

        let mut field = |axis: usize, value: u32| {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
            // Applied when the edit ends: each new size is a new session.
            let events = cx.subscribe(
                &input,
                move |this: &mut Self, input, event: &InputEvent, cx| {
                    if !matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        return;
                    }
                    let Ok(typed) = input.read(cx).value().trim().parse::<u32>() else {
                        return;
                    };

                    let range = match axis {
                        0 => rox_services::renderers::WIDTHS,
                        _ => rox_services::renderers::HEIGHTS,
                    };
                    let pixels = typed.clamp(*range.start(), *range.end());
                    match axis {
                        0 => this.config.width = pixels,
                        _ => this.config.height = pixels,
                    }
                    cx.notify();
                },
            );
            (input, events)
        };

        let inputs = [field(0, self.config.width), field(1, self.config.height)];
        let both = [inputs[0].0.clone(), inputs[1].0.clone()];
        self.size_inputs = Some((window.window_handle(), inputs));
        both
    }
}

/// The cover a session should hold now. None between two tracks, where the
/// old cover holds, as a canvas target's does.
fn playing_cover(state: &AppState, cx: &App) -> Option<CoverAsk> {
    let player = state.player.read(cx);
    let now = player.now_playing();
    if now.is_none() && player.is_active() && !player.queue_ended() {
        return None;
    }

    let track = now.map(|now| now.key.path);
    let store = state.thumbs.read(cx).store_conn();
    Some(CoverAsk::new(track, store))
}

impl PanelSettings for RendererPanel {
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
        &[("Source", icons::MONITOR_PLAY)]
    }

    fn page_body(
        &mut self,
        _page: &'static str,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageBody {
        let [width_input, height_input] = self.size_inputs(window, cx);

        // A saved renderer this machine doesn't have stays picked, by its id.
        let mut rows: Vec<(String, SharedString)> =
            target::renderer_choices(&rox_services::renderers::all())
                .into_iter()
                .map(|choice| (choice.owner.clone(), choice.shown()))
                .collect();
        let owner = self.config.chrome.owner.clone();
        if !rows.iter().any(|(listed, _)| *listed == owner) {
            let shown = match target::renderer_id(&owner) {
                Some(id) => rox_i18n::t!("canvas-target-not-installed", id = id),
                None => rox_i18n::t!("canvas-target-renderer-none"),
            };
            rows.insert(0, (owner.clone(), shown));
        }

        // One that isn't switched on raises its card in the panel's own
        // window, where it draws, and that window comes forward.
        let renderer = picker(
            "renderer-panel-renderer",
            owner,
            rows,
            false,
            |this: &mut Self, picked: String, cx| {
                if this.choose(picked, cx) {
                    this.ask_enable_later(cx);
                }
            },
            cx,
        );

        let sizings = [
            (rox_i18n::t!("canvas-target-panel-fixed"), Sizing::Fixed),
            (rox_i18n::t!("canvas-target-panel-match"), Sizing::Panel),
        ];
        let sizing = choices_shared(
            &sizings,
            self.config.sizing,
            |this: &mut Self, sizing, cx| {
                this.config.sizing = sizing;
                cx.notify();
            },
            cx,
        );

        let scalings = [
            (rox_i18n::t!("canvas-target-panel-fit"), Scaling::Fit),
            (rox_i18n::t!("canvas-target-panel-fill"), Scaling::Fill),
        ];
        let scaling = choices_shared(
            &scalings,
            self.config.scaling,
            |this: &mut Self, scaling, cx| {
                this.config.scaling = scaling;
                cx.notify();
            },
            cx,
        );

        let presets: Vec<(SharedString, [u32; 2])> = crate::canvas::overlay::TARGET_PRESETS
            .iter()
            .map(|(label, size)| (SharedString::from(*label), *size))
            .collect();
        let chips = choices_shared(
            &presets,
            [self.config.width, self.config.height],
            |this: &mut Self, [width, height]: [u32; 2], cx| {
                this.config.width = width;
                this.config.height = height;
                this.size_inputs = None;
                cx.notify();
            },
            cx,
        );
        let size_fields = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(div().w(px(72.)).child(Input::new(&width_input).small()))
            .child(div().text_color(palette::text_muted()).child("x"))
            .child(div().w(px(72.)).child(Input::new(&height_input).small()));

        let fixed = self.config.sizing == Sizing::Fixed;
        let rests = toggle(
            self.config.rests,
            |this: &mut Self, on, cx| {
                this.config.rests = on;
                cx.notify();
            },
            cx,
        );

        PageBody::new().section(Section::new(
            q,
            icons::MONITOR_PLAY,
            rox_i18n::t!("panel-catalog-renderer"),
            None,
            |rows| {
                rows.keyed(
                    "canvas-target-panel-renderer",
                    &["renderer", "program", "draw"],
                    renderer,
                )
                .keyed(
                    "canvas-target-panel-sizing",
                    &["resolution", "size", "pixels", "match"],
                    sizing,
                )
                .when(fixed, |rows| {
                    rows.keyed(
                        "canvas-target-resolution",
                        &["resolution", "size", "720p", "1080p", "4k"],
                        chips,
                    )
                    .keyed(
                        "canvas-target-panel-size",
                        &["width", "height", "pixels"],
                        size_fields,
                    )
                    .keyed(
                        "canvas-target-panel-scaling",
                        &["fit", "fill", "crop", "letterbox"],
                        scaling,
                    )
                })
                .keyed(
                    "canvas-target-rests",
                    &["pause", "music", "stop", "idle"],
                    rests,
                )
            },
        ))
    }
}

impl EventEmitter<PanelEvent> for RendererPanel {}

impl Focusable for RendererPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for RendererPanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let named = self
            .renderer_name()
            .map(SharedString::from)
            .unwrap_or_else(|| rox_i18n::t!("panel-catalog-renderer"));
        panel::title_text(self.config.chrome.title.as_deref(), named)
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

    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
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
        self.menu = None;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.menu = None;
        self.leave();

        // A Pop Out takes the session along, as a canvas's targets go: one on
        // shared memory carries on, and one lent a GPU texture starts again
        // with one made over there. Closed, it stops.
        match rox_dock::panel_moving() {
            true => {
                if let Some(shown) = &self.shown {
                    shown.carry(window, cx);
                }
            }
            false => {
                if let Some(shown) = self.shown.take() {
                    shown.release(window, cx);
                }
            }
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let renderers = self.renderer_menu(window, cx);
        let menu = menu.item(
            PopupMenuItem::submenu(rox_i18n::t!("canvas-target-renderer"), renderers)
                .icon(Icon::default().path(icons::MONITOR_PLAY)),
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
                RendererPanel::new(state, config, cx)
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

impl Render for RendererPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window = Some(window.window_handle());
        self.read(cx);
        let picture = self.picture(window, cx);
        let empty = self.empty_state(cx);
        let menu = self.menu_layer(window);
        let prompt = self.prompt_layer(window, cx);

        // Bounds land a frame late, so a resize renders once more when they
        // move. gpui drops a notify made while the window draws.
        let me = cx.entity_id();
        let captured = self.bounds.clone();
        let pressed = self.pressed.clone();
        let watch = canvas(
            move |bounds, _, cx| {
                if captured.get() != bounds {
                    captured.set(bounds);
                    cx.defer(move |cx| cx.notify(me));
                }
            },
            move |_, _, window, _| {
                // A button held anywhere over the window is a drag going on.
                let down = pressed.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
                    if phase.bubble() {
                        down.set(true);
                    }
                });
                let up = pressed.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
                    if phase.bubble() {
                        up.set(false);
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let body = div()
            .id("renderer-panel")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                let at = this.local(event.position);
                this.point(at, Pointer::Move);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                if !hovered {
                    this.leave();
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, _| {
                let at = this.local(event.position);
                let lines = scroll_lines(event.delta);
                this.point(at, |spot| Pointer::Scroll(spot, lines));
            }));

        // The right button is the panel's: its menu opens through the tab
        // group, and the renderer never sees that press.
        let buttons = [
            MouseButton::Left,
            MouseButton::Middle,
            MouseButton::Navigate(NavigationDirection::Back),
            MouseButton::Navigate(NavigationDirection::Forward),
        ];
        let body = buttons
            .into_iter()
            .fold(body, |body, button| {
                body.on_mouse_down(
                    button,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        let at = this.local(event.position);
                        this.point(at, |spot| Pointer::Down(spot, button_name(button)));
                    }),
                )
                .on_mouse_up(
                    button,
                    cx.listener(move |this, event: &MouseUpEvent, _, _| {
                        let at = this.local(event.position);
                        this.point(at, |spot| Pointer::Up(spot, button_name(button)));
                    }),
                )
            })
            .child(watch)
            .children(picture)
            .children(empty)
            .children(menu)
            .children(prompt);

        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, move || div().size_full().child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    #[test]
    fn fit_letterboxes_and_keeps_the_whole_picture() {
        // A 16:9 picture in a square panel: bands above and below.
        let fit = layout(Sizing::Fixed, Scaling::Fit, [1280, 720], [400., 400.]);
        assert!(close(fit.rect, [0., 87.5, 400., 225.]), "{:?}", fit.rect);
        assert_eq!(fit.crop, WHOLE);

        // In a wide panel: bands at the sides.
        let wide = layout(Sizing::Fixed, Scaling::Fit, [1280, 720], [1000., 200.]);
        let width = 200. * 1280. / 720.;
        assert!(close(wide.rect, [(1000. - width) / 2., 0., width, 200.]));
    }

    #[test]
    fn fill_covers_the_panel_and_crops_the_middle() {
        let fill = layout(Sizing::Fixed, Scaling::Fill, [1280, 720], [400., 400.]);
        assert!(close(fill.rect, [0., 0., 400., 400.]));

        // A square panel on 16:9 keeps the middle 9/16 across.
        let shown = 720. / 1280.;
        assert!((fill.crop.size.width - shown).abs() < 1e-4);
        assert!((fill.crop.origin.x - (1. - shown) / 2.).abs() < 1e-4);
        assert_eq!(fill.crop.size.height, 1.);
    }

    #[test]
    fn matching_the_panel_stretches_while_it_settles() {
        let matched = layout(Sizing::Panel, Scaling::Fit, [1280, 720], [300., 300.]);
        assert!(close(matched.rect, [0., 0., 300., 300.]));
        assert_eq!(matched.crop, WHOLE);
    }

    #[test]
    fn the_pointer_lands_on_the_picture_not_the_bands() {
        let fit = layout(Sizing::Fixed, Scaling::Fit, [1280, 720], [400., 400.]);
        assert_eq!(surface_point(&fit, [200., 200.]), Some([0.5, 0.5]));
        assert_eq!(surface_point(&fit, [0., 87.5]), Some([0., 0.]));
        assert_eq!(surface_point(&fit, [200., 40.]), None, "over the top band");

        let fill = layout(Sizing::Fixed, Scaling::Fill, [1280, 720], [400., 400.]);
        let [x, y] = surface_point(&fill, [0., 400.]).unwrap();
        assert!(
            (x - fill.crop.origin.x).abs() < 1e-4,
            "the crop's left edge"
        );
        assert_eq!(y, 1.);
    }

    #[test]
    fn a_matched_size_is_device_pixels_held_to_the_limits_and_even() {
        assert_eq!(matched_size([641., 361.], 1.), [640, 360]);
        assert_eq!(matched_size([640.5, 360.], 2.), [1280, 720]);
        assert_eq!(
            matched_size([4., 3.], 1.),
            [16, 16],
            "the smallest a renderer draws"
        );
        assert_eq!(
            matched_size([3000., 2000.], 2.),
            [3840, 2160],
            "and the largest"
        );
    }

    #[test]
    fn a_new_panel_is_fixed_720p_fitted_and_runs_through_pauses() {
        let config = RendererConfig::default();
        assert_eq!(config.sizing, Sizing::Fixed);
        assert_eq!([config.width, config.height], [1280, 720]);
        assert_eq!(config.scaling, Scaling::Fit);
        assert!(!config.rests);
        assert!(config.chrome.owner.is_empty());
    }

    #[test]
    fn the_config_saves_the_renderer_as_its_owner() {
        let config = RendererConfig {
            chrome: PanelChrome {
                owner: "plugin:bars".into(),
                ..PanelChrome::default()
            },
            sizing: Sizing::Panel,
            scaling: Scaling::Fill,
            rests: true,
            ..RendererConfig::default()
        };

        let saved = serde_json::to_value(&config).unwrap();
        assert_eq!(saved["owner"], "plugin:bars");
        assert_eq!(saved["sizing"], "panel");
        assert_eq!(saved["scaling"], "fill");

        let back: RendererConfig = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(back.chrome.owner, "plugin:bars");
        assert!(back.rests);

        // A layout holding one lists the renderer, and names it a renderer.
        let dump = serde_json::json!({
            "panel_name": PANEL_NAME,
            "children": [],
            "info": { "panel": saved },
        });
        let mut named = std::collections::BTreeSet::new();
        target::named_renderers(&dump, &mut named);
        assert_eq!(named.into_iter().collect::<Vec<_>>(), ["bars"]);
    }
}
