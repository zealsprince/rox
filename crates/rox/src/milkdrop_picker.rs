//! The Milkdrop preset picker: the preset browser over whatever opened it,
//! the backdrop or a Milkdrop panel. A click puts the preset up on the host
//! at once, so the preview is the real thing. One window at a time: opening
//! for another host retargets it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, EntityId, Global, Subscription, WeakEntity, Window, WindowHandle,
    div, prelude::*, size,
};
use gpui_component::{Root, Sizable as _};

use rox_core::settings::{self as core_settings, MilkdropPickerWindowState, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_api::preset_browser::{BrowserEvent, PresetBrowser, PresetHost, preset_label};
use rox_panel_kit::ui as settings_ui;

struct OpenPicker {
    handle: WindowHandle<Root>,
    view: WeakEntity<PickerWindow>,
}

impl Global for OpenPicker {}

/// Deferred: a panel's menu action runs inside the workspace's update, and
/// reading the front workspace for the tint mid-update would panic.
pub fn open(host: Box<dyn PresetHost>, cx: &mut App) {
    cx.defer(move |cx| open_now(host, cx));
}

fn open_now(host: Box<dyn PresetHost>, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenPicker>()
        && let Some(view) = open.view.upgrade()
    {
        let handle = open.handle;
        view.update(cx, |this, cx| this.retarget(host, cx));
        handle
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    open_fresh_with(host, cx);
}

fn open_fresh_with(host: Box<dyn PresetHost>, cx: &mut App) {
    let player =
        rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state.player.entity_id());
    let min = settings_ui::MIN_SIZE;
    let saved = Settings::load().windows.milkdrop_picker;
    let (width, height) = saved
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((560., 640.));
    let switches = saved.unwrap_or_default();
    let bounds = Bounds::centered(None, size(gpui::px(width), gpui::px(height)), cx);
    let built: Rc<RefCell<Option<WeakEntity<PickerWindow>>>> = Rc::new(RefCell::new(None));
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("milkdrop-picker-title"),
        bounds,
        Some(min),
        {
            let built = built.clone();
            move |window, cx| {
                let view = cx.new(|cx| PickerWindow::new(host, player, switches, window, cx));
                *built.borrow_mut() = Some(view.downgrade());
                view
            }
        },
    );
    let view = built.borrow_mut().take();
    if let Some(view) = view {
        cx.set_global(OpenPicker { handle, view });
    }
}

struct PickerWindow {
    host: Box<dyn PresetHost>,
    browser: Entity<PresetBrowser>,
    /// None themes to the base palette.
    player: Option<EntityId>,
    /// The lists edit the presets were last handed in at, so it happens on change.
    lists_gen: Option<u64>,
    /// The host went away; the next render closes the window.
    orphaned: bool,
    _browser_events: Subscription,
    _host_events: Vec<Subscription>,
}

impl PickerWindow {
    fn new(
        host: Box<dyn PresetHost>,
        player: Option<EntityId>,
        switches: MilkdropPickerWindowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The OS close button never runs remove_window, so the size persists here.
        // The switches write as they flip, so this edits the entry in place.
        window.on_window_should_close(cx, |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let saved = s
                    .windows
                    .milkdrop_picker
                    .get_or_insert_with(MilkdropPickerWindowState::default);
                saved.width = frame.size.width.into();
                saved.height = frame.size.height.into();
            });
            true
        });
        let browser = cx.new(|cx| {
            let mut browser = PresetBrowser::new(None, window, cx);
            browser.set_thumbs(rox_panels::milkdrop::thumbnails(), cx);
            browser.set_favorites_only(switches.favorites_only, cx);
            browser.set_nested(switches.nested, cx);
            browser
        });
        let _browser_events =
            cx.subscribe(&browser, |this, _, event: &BrowserEvent, cx| match event {
                BrowserEvent::Pick(path) => {
                    this.host.pick(path.clone(), cx);
                    cx.notify();
                }
                BrowserEvent::Switched {
                    favorites_only,
                    nested,
                } => {
                    let (favorites_only, nested) = (*favorites_only, *nested);
                    Settings::update(move |s| {
                        let saved = s
                            .windows
                            .milkdrop_picker
                            .get_or_insert_with(MilkdropPickerWindowState::default);
                        saved.favorites_only = favorites_only;
                        saved.nested = nested;
                    });
                }
            });
        let _host_events = Self::watch(&*host, cx);
        PickerWindow {
            host,
            browser,
            player,
            lists_gen: None,
            orphaned: false,
            _browser_events,
            _host_events,
        }
    }

    fn watch(host: &dyn PresetHost, cx: &mut Context<Self>) -> Vec<Subscription> {
        let wake = cx.entity().downgrade();
        let gone = cx.entity().downgrade();
        host.watch(
            Rc::new(move |cx: &mut App| {
                wake.update(cx, |_, cx| cx.notify()).ok();
            }),
            Rc::new(move |cx: &mut App| {
                gone.update(cx, |this, cx| {
                    this.orphaned = true;
                    cx.notify();
                })
                .ok();
            }),
            cx,
        )
    }

    /// The browser keeps its filter and folds across a retarget.
    fn retarget(&mut self, host: Box<dyn PresetHost>, cx: &mut Context<Self>) {
        self._host_events = Self::watch(&*host, cx);
        self.host = host;
        self.lists_gen = None;
        self.orphaned = false;
        cx.notify();
    }

    fn follow_host(&mut self, cx: &mut Context<Self>) {
        let generation = core_settings::milkdrop_gen();
        if self.lists_gen != Some(generation) {
            self.lists_gen = Some(generation);
            let presets = self.host.presets(cx);
            self.browser
                .update(cx, |browser, cx| browser.set_presets(&presets, cx));
        }
        let current = self.host.current(cx);
        self.browser
            .update(cx, |browser, cx| browser.set_current(current, cx));
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let current = self.host.current(cx);
        let has_current = current.is_some();
        let showing = current
            .as_deref()
            .map(preset_label)
            .map(gpui::SharedString::from)
            .unwrap_or_else(|| rox_i18n::t!("milkdrop-no-preset"));
        let reveal = current.clone();
        let starred = current
            .as_deref()
            .is_some_and(core_settings::is_milkdrop_favorite);
        let star = current.clone();
        let favorite = div()
            .id("milkdrop-picker-favorite")
            .flex_none()
            .when(has_current, |star| star.cursor_pointer())
            .child(
                gpui_component::Icon::default()
                    .path(if starred {
                        icons::STAR_FILLED
                    } else {
                        icons::STAR
                    })
                    .small()
                    .text_color(if starred {
                        palette::accent()
                    } else if has_current {
                        palette::text_muted()
                    } else {
                        palette::text_faint()
                    }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |_, _, _, cx| {
                    if let Some(path) = star.as_deref() {
                        let on = !core_settings::is_milkdrop_favorite(path);
                        core_settings::set_milkdrop_favorite(path, on);
                        cx.notify();
                    }
                }),
            );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_b_1()
            .border_color(palette::border())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "milkdrop-picker-host",
                        host = self.host.title(cx).to_string()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(favorite)
                    .child(div().flex_1().min_w_0().truncate().child(showing))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("milkdrop-random"),
                        icons::SHUFFLE,
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.host.random(cx);
                            cx.notify();
                        }),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("milkdrop-reveal-short"),
                        icons::EXTERNAL_LINK,
                        !has_current,
                        move |_, _, cx| {
                            if let Some(path) = reveal.as_deref() {
                                cx.reveal_path(path);
                            }
                        },
                    )),
            )
    }

    fn empty(&self) -> impl IntoElement + use<> {
        let folder = core_settings::milkdrop_dir().join("presets");
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .child(
                div()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("milkdrop-empty")),
            )
            .child(settings_ui::small_button(
                rox_i18n::t!("milkdrop-open-folder"),
                icons::FOLDER,
                false,
                move |_, _, cx| {
                    // Nothing else creates the folder, and the platform refuses a missing path.
                    std::fs::create_dir_all(&folder).ok();
                    cx.open_with_system(&folder);
                },
            ))
    }
}

impl Render for PickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.orphaned {
            window.remove_window();
            return div().into_any_element();
        }
        self.follow_host(cx);
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        let empty = self.browser.read(cx).total() == 0;
        // Build inside the closure: anything built before it reads the untinted palette.
        panel::window_body(player, || {
            let header = self.header(cx);
            let body = if empty {
                self.empty().into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .p(tokens::SPACE_MD)
                    .child(self.browser.clone())
                    .into_any_element()
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(header)
                .child(body)
                .into_any_element()
        })
        .into_any_element()
    }
}
