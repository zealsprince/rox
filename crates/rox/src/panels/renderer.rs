//! The Renderer panel (ADR 37, amended 2026-11-26): one renderer drawing the
//! whole panel, docked, tabbed and popped out like any other. It's a second
//! host for the canvas's render target, through the same [`Shown`] session,
//! supervisor, frame paths and grants, so a renderer can't tell the two
//! apart.
//!
//! The renderer is named the way a plugin's panel names its plugin, as
//! `plugin:<id>` in the chrome's `owner`, so a saved layout lists it in
//! `requires`. Following the panel, the session draws at the panel's device
//! pixels times its render scale, and a renderer that resizes hears a new
//! size once a resize has held still, without starting again. One that
//! doesn't keeps the size it started at, fitted into the panel. A fixed
//! resolution is drawn the same way. The renderer's own settings are stored
//! here, with the panel. The pointer goes to the renderer except the right
//! button, which opens the panel's menu. Nothing here approves or switches
//! anything on: a renderer that isn't ready raises its enable card, and only
//! the card's Switch On approves.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, DismissEvent, Entity, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, Pixels, Point, ScrollWheelEvent, SharedString, Subscription, WeakEntity,
    Window, anchored, canvas, deferred, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_canvas::{ImageFit, Projected};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_dock::{Panel, PanelEvent, PanelState, TabPanel};
use rox_panel_api::panel::{
    self, AppState, PanelChrome, PanelSettings, Query, ScrubState, ValueEdit,
};
use rox_panel_api::panel_settings;
use rox_panel_kit::ui::{self as settings_ui, PageBody, Section};
use rox_panel_kit::{choices_shared, picker, toggle};
use serde::{Deserialize, Serialize};

use crate::canvas::target::{
    self, Ask, CoverAsk, Fix, Key, Place, Pointer, Readiness, Reading, Shown,
};
use crate::canvas::target_settings::{self, fit_choices, scale_choices};
use crate::enable_card::EnableCard;
use crate::panels::canvas::{RENDERERS_PAGE, button_name, scroll_lines, suggests_rest};

/// The registry name.
pub const PANEL_NAME: &str = "renderer";

/// A fixed resolution's starting value.
const DEFAULT_SIZE: [u32; 2] = [1280, 720];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sizing {
    /// A resolution of the panel's own.
    Fixed,
    /// The panel's size in device pixels, times the render scale.
    #[default]
    Panel,
}

/// How the picture fits the panel when their shapes differ. Saved under the
/// names the panel first had for the first two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scaling {
    /// All of the picture, with the panel's background in the bands.
    Fit,
    /// The whole panel covered, the picture's overflow cropped.
    #[default]
    Fill,
    /// The whole panel covered, the picture squeezed to it.
    Stretch,
}

impl Scaling {
    fn fit(self) -> ImageFit {
        match self {
            Scaling::Fit => ImageFit::Contain,
            Scaling::Fill => ImageFit::Cover,
            Scaling::Stretch => ImageFit::Stretch,
        }
    }

    fn of(fit: ImageFit) -> Scaling {
        match fit {
            ImageFit::Contain => Scaling::Fit,
            ImageFit::Cover => Scaling::Fill,
            ImageFit::Stretch => Scaling::Stretch,
        }
    }
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
    /// Following the panel, its pixels as a percent of the panel's.
    pub scale: u32,
    /// Pauses the renderer while the music is paused or stopped.
    pub rests: bool,
    /// The renderer's settings by key, as its manifest declares them.
    pub settings: serde_json::Value,
    /// The settings of each renderer the panel showed before, by its id,
    /// so picking it again finds them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub aside: BTreeMap<String, serde_json::Value>,
}

impl Default for RendererConfig {
    fn default() -> Self {
        RendererConfig {
            chrome: PanelChrome::default(),
            sizing: Sizing::Panel,
            width: DEFAULT_SIZE[0],
            height: DEFAULT_SIZE[1],
            scaling: Scaling::Fill,
            scale: 100,
            rests: false,
            settings: serde_json::Value::Null,
            aside: BTreeMap::new(),
        }
    }
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
    /// A button held down over the window: a drag, which a following size
    /// waits out.
    pressed: Rc<Cell<bool>>,
    menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    prompt: Option<Prompt>,
    /// The fixed resolution's typed fields and the settings window they
    /// were made in: another settings window makes its own.
    size_inputs: Option<SizeInputs>,
    /// The settings' number strips, by setting key, and their typing.
    scrubs: HashMap<String, ScrubState>,
    value_edit: ValueEdit,
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
            pressed: Rc::default(),
            menu: None,
            prompt: None,
            size_inputs: None,
            scrubs: HashMap::new(),
            value_edit: ValueEdit::default(),
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
            let config = &mut self.config;
            target_settings::switch(
                &mut config.settings,
                &mut config.aside,
                &config.chrome.owner,
                &owner,
            );
            self.config.chrome.owner = owner;
            panel::refresh_tab_panel(&self.tab_panel, cx);
            self.save_config(cx);
        }

        cx.notify();
        off
    }

    /// A setting moved: the layout is saved soon, as a dock change is. The
    /// panel's own events never reach the dock, so it goes through the tab
    /// panel, and a pick lands even if rox is killed a moment later.
    fn save_config(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(WeakEntity::upgrade) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    /// One edit of the config from the settings page, saved and shown.
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut RendererConfig)) {
        f(&mut self.config);
        self.save_config(cx);
        cx.notify();
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

        let size = match self.config.sizing {
            Sizing::Fixed => target::Sizing::Fixed([self.config.width, self.config.height]),
            Sizing::Panel => target::Sizing::Follow(self.config.scale),
        };
        let key = Key { owner, size };
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

        let device = window.scale_factor();
        let ceiling = match size {
            target::Sizing::Follow(percent) => target::ceiling(cx, device, percent),
            target::Sizing::Fixed(size) => size,
        };
        let place = Place {
            face: Projected {
                corners: [
                    [0., 0.],
                    [panel[0], 0.],
                    [panel[0], panel[1]],
                    [0., panel[1]],
                ],
                depth: 0.,
                scale: 1.,
                opacity: 1.,
            },
            room: panel.map(|side| side * device),
            fit: self.config.scaling.fit(),
            pressed: self.pressed.get(),
            ceiling,
            settings: &self.config.settings,
        };
        let rest = self.config.rests && !self.reading.playing();

        let shown = self.shown.get_or_insert_with(|| Shown::new(key));
        let (view, _) = shown.paint(place, rest, self.reading.feed(), cx);
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
        let size = self.bounds.get().size;
        let room = [f32::from(size.width), f32::from(size.height)];
        if room[0] <= 0. || room[1] <= 0. {
            return;
        }

        let across = [at[0] / room[0], at[1] / room[1]];
        self.aim(pointer(across));
    }

    fn leave(&mut self) {
        self.aim(Pointer::Leave);
    }

    fn aim(&mut self, pointer: Pointer) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };

        if let Some(pointer) = shown.aim(pointer)
            && let Some(target) = shown.target()
        {
            target.point(pointer);
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
        let installed = status.is_some() || !rox_services::renderers::all().is_empty();
        let target::Prompt { words, ask } = target::prompt_for(status.as_ref(), installed)?;
        let button = ask.map(|ask| target::ask_button(ask, Self::answer, cx));

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

    fn answer(&mut self, ask: Ask, window: &mut Window, cx: &mut Context<Self>) {
        match ask {
            Ask::Pick => {
                let at = window.mouse_position();
                self.open_picker(at, window, cx)
            }
            Ask::Fix(fix) => self.fix(fix, window, cx),
        }
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
                    this.edit(cx, |config| match axis {
                        0 => config.width = pixels,
                        _ => config.height = pixels,
                    });
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
            (rox_i18n::t!("canvas-target-panel-match"), Sizing::Panel),
            (rox_i18n::t!("canvas-target-panel-fixed"), Sizing::Fixed),
        ];
        let sizing = choices_shared(
            &sizings,
            self.config.sizing,
            |this: &mut Self, sizing, cx| this.edit(cx, |config| config.sizing = sizing),
            cx,
        );

        let scale = choices_shared(
            &scale_choices(),
            self.config.scale,
            |this: &mut Self, scale, cx| this.edit(cx, |config| config.scale = scale),
            cx,
        );

        let fit = choices_shared(
            &fit_choices(),
            self.config.scaling.fit(),
            |this: &mut Self, fit, cx| this.edit(cx, |config| config.scaling = Scaling::of(fit)),
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
                this.size_inputs = None;
                this.edit(cx, |config| {
                    config.width = width;
                    config.height = height;
                });
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
            |this: &mut Self, on, cx| this.edit(cx, |config| config.rests = on),
            cx,
        );

        let settings = self.setting_rows(cx);

        let page = PageBody::new().section(Section::new(
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
                    &["resolution", "size", "pixels", "follow", "match"],
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
                })
                .when(!fixed, |rows| {
                    rows.keyed(
                        "canvas-target-scale",
                        &["scale", "resolution", "sharp", "percent"],
                        scale,
                    )
                })
                .keyed(
                    "canvas-target-fit",
                    &["fit", "fill", "crop", "letterbox", "stretch"],
                    fit,
                )
                .keyed(
                    "canvas-target-rests",
                    &["pause", "music", "stop", "idle"],
                    rests,
                )
            },
        ));

        if settings.is_empty() {
            return page;
        }
        page.section(Section::new(
            q,
            icons::SETTINGS,
            rox_i18n::t!("canvas-target-settings"),
            None,
            |rows| {
                settings.into_iter().fold(rows, |rows, row| {
                    let title = row.title.clone();
                    rows.row_dyn(&[title.as_ref()], row.title, None, row.control)
                })
            },
        ))
    }
}

impl RendererPanel {
    /// A row for each setting the picked renderer declares, the values
    /// stored with this panel.
    fn setting_rows(&mut self, cx: &mut Context<Self>) -> Vec<target_settings::SettingRow> {
        let Some(id) = self.renderer() else {
            return Vec::new();
        };
        let Some(renderer) = rox_services::renderers::all()
            .into_iter()
            .find(|renderer| renderer.id == id)
        else {
            return Vec::new();
        };

        let set = |this: &mut Self, key: &str, value: serde_json::Value, cx: &mut Context<Self>| {
            this.edit(cx, |config| {
                config.settings = target_settings::with(&config.settings, key, value);
            });
        };

        let scrubs = &mut self.scrubs;
        let mut scrub = |key: &str| scrubs.entry(key.to_string()).or_default().clone();
        target_settings::setting_rows(
            &renderer.settings,
            &self.config.settings,
            &mut scrub,
            &self.value_edit,
            set,
            cx,
        )
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

    #[test]
    fn a_new_panel_follows_the_panel_covered_and_runs_through_pauses() {
        let config = RendererConfig::default();
        assert_eq!(config.sizing, Sizing::Panel);
        assert_eq!(config.scale, 100);
        assert_eq!(config.scaling, Scaling::Fill);
        assert_eq!(config.scaling.fit(), ImageFit::Cover);
        assert!(!config.rests);
        assert!(config.chrome.owner.is_empty());
        assert!(config.settings.is_null());
    }

    #[test]
    fn a_panel_saved_before_the_render_scale_reads_as_it_was() {
        let saved = serde_json::json!({
            "owner": "plugin:bars",
            "sizing": "fixed",
            "width": 1920,
            "height": 1080,
            "scaling": "fit",
            "rests": false,
        });
        let config: RendererConfig = serde_json::from_value(saved).unwrap();

        assert_eq!(config.sizing, Sizing::Fixed);
        assert_eq!([config.width, config.height], [1920, 1080]);
        assert_eq!(config.scaling.fit(), ImageFit::Contain);
        assert_eq!(config.scale, 100);
    }

    #[test]
    fn a_panel_saved_before_renderers_kept_their_own_settings_reads_as_it_was() {
        let saved = serde_json::json!({
            "owner": "plugin:plasma",
            "settings": { "look": "ember", "rate": 30 },
        });
        let config: RendererConfig = serde_json::from_value(saved).unwrap();

        assert_eq!(
            config.settings,
            serde_json::json!({ "look": "ember", "rate": 30 })
        );
        assert!(config.aside.is_empty());
        let written = serde_json::to_value(&config).unwrap();
        assert!(
            written.get("aside").is_none(),
            "nothing aside is nothing written"
        );
    }

    #[test]
    fn the_config_saves_the_renderer_as_its_owner() {
        let config = RendererConfig {
            chrome: PanelChrome {
                owner: "plugin:bars".into(),
                ..PanelChrome::default()
            },
            sizing: Sizing::Panel,
            scaling: Scaling::Stretch,
            scale: 150,
            rests: true,
            settings: serde_json::json!({ "look": "ice" }),
            ..RendererConfig::default()
        };

        let saved = serde_json::to_value(&config).unwrap();
        assert_eq!(saved["owner"], "plugin:bars");
        assert_eq!(saved["sizing"], "panel");
        assert_eq!(saved["scaling"], "stretch");
        assert_eq!(saved["scale"], 150);
        assert_eq!(saved["settings"]["look"], "ice");

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
