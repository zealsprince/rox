//! The slide panel: a carousel of panels in one slot, one up at a time, with
//! arrows, dots and a draggable rail. Hosted through [`crate::composite`]; only
//! the slides touching the viewport render.

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta, ScrollWheelEvent, SharedString, WeakEntity,
    Window, canvas, div, linear_color_stop, linear_gradient, prelude::*, px, relative,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::composite::{self, Slot};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::ScrubState;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlideConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub active: usize,
}

const RAIL_H: Pixels = px(24.);

const SCRIM_W: Pixels = px(26.);

/// Rail travel that turns a press into a drag, in viewport widths, so a twitchy
/// tap never half-drags the deck.
const DRAG_DEAD_ZONE: f32 = 0.015;

struct RailDrag {
    start_frac: f32,
    start_pos: f32,
    pos: f32,
    /// Past the dead zone, so the release snaps; a click's glide is left alone.
    moved: bool,
}

pub struct SlidePanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: SlideConfig,
    slides: Vec<Arc<dyn PanelView>>,
    from: f32,
    slide_at: Instant,
    rail: ScrubState,
    drag: Option<RailDrag>,
    /// Wheel travel pooled in lines, so a trackpad's trickle steps one slide
    /// per notch's worth.
    wheel: f32,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    introduced: bool,
}

impl SlidePanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: SlideConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    /// Empty sentinels from a hand-edited dump drop out, and the active index
    /// re-clamps.
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        mut config: SlideConfig,
        slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let slides: Vec<Arc<dyn PanelView>> = slots.into_iter().flatten().collect();
        config.active = config.active.min(slides.len().saturating_sub(1));
        let from = config.active as f32;
        SlidePanel {
            state,
            workspace,
            config,
            slides,
            from,
            slide_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            rail: ScrubState::default(),
            drag: None,
            wheel: 0.0,
            focus: cx.focus_handle(),
            tab_panel: None,
            introduced: false,
        }
    }

    pub fn slides(&self) -> &[Arc<dyn PanelView>] {
        &self.slides
    }

    fn pos(&self) -> f32 {
        if let Some(drag) = &self.drag {
            return drag.pos;
        }
        let u = (self.slide_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let u = u * u * (3.0 - 2.0 * u);
        self.from + (self.config.active as f32 - self.from) * u
    }

    /// A dot under the press has already jumped (children bubble first). The
    /// notify arms the window-level drag handlers on the next paint.
    fn begin_rail_drag(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(frac) = self.rail.fraction(x) else {
            return;
        };
        let pos = self.pos();
        self.rail.begin();
        self.drag = Some(RailDrag {
            start_frac: frac,
            start_pos: pos,
            pos,
            moved: false,
        });
        cx.notify();
    }

    fn rail_drag_to(&mut self, frac: f32, cx: &mut Context<Self>) {
        let count = self.slides.len();
        let Some(drag) = &mut self.drag else {
            return;
        };
        let delta = drag.start_frac - frac;
        if delta.abs() > DRAG_DEAD_ZONE {
            drag.moved = true;
        }
        if !drag.moved {
            return;
        }
        drag.pos = (drag.start_pos + delta).clamp(0.0, count.saturating_sub(1) as f32);
        cx.notify();
    }

    /// A release inside the dead zone was a click; its glide keeps running.
    fn end_rail_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rail.end();
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.moved {
            cx.notify();
            return;
        }
        let target = (drag.pos.round() as usize).min(self.slides.len().saturating_sub(1));
        // Not go(): a same-slide target still needs the glide re-based on the
        // dragged position.
        if target != self.config.active {
            if let Some(child) = self.slides.get(self.config.active) {
                child.set_active(false, window, cx);
            }
            if let Some(child) = self.slides.get(target) {
                child.set_active(true, window, cx);
            }
            self.config.active = target;
        }
        self.from = drag.pos;
        self.slide_at = Instant::now();
        cx.notify();
    }

    fn go(&mut self, target: usize, window: &mut Window, cx: &mut Context<Self>) {
        let target = target.min(self.slides.len().saturating_sub(1));
        if target == self.config.active {
            return;
        }
        // Hand the active toggle to the arriving slide, so a visualizer that
        // scrolls off stops working. The panel's own set_active only reaches
        // the shown slide.
        if let Some(child) = self.slides.get(self.config.active) {
            child.set_active(false, window, cx);
        }
        self.from = self.pos();
        self.config.active = target;
        if let Some(child) = self.slides.get(self.config.active) {
            child.set_active(true, window, cx);
        }
        self.slide_at = Instant::now();
        cx.notify();
    }

    fn snap(&mut self, cx: &mut Context<Self>) {
        self.from = self.config.active as f32;
        self.slide_at = Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS);
        cx.notify();
    }

    fn add(&mut self, panel: Arc<dyn PanelView>, window: &mut Window, cx: &mut Context<Self>) {
        self.slides.push(panel);
        self.introduced = false;
        if self.slides.len() == 1 {
            self.snap(cx);
            if let Some(child) = self.slides.get(self.config.active) {
                child.set_active(true, window, cx);
            }
        } else {
            self.go(self.slides.len() - 1, window, cx);
        }
    }

    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.slides.len() {
            return;
        }
        self.slides.remove(ix);
        self.introduced = false;
        self.config.active = self.config.active.min(self.slides.len().saturating_sub(1));
        self.snap(cx);
    }

    fn replace(&mut self, ix: usize, panel: Arc<dyn PanelView>, cx: &mut Context<Self>) {
        if ix >= self.slides.len() {
            return;
        }
        self.slides[ix] = panel;
        cx.notify();
    }

    fn shift(&mut self, ix: usize, right: bool, cx: &mut Context<Self>) {
        let other = if right { ix + 1 } else { ix.wrapping_sub(1) };
        if ix >= self.slides.len() || other >= self.slides.len() {
            return;
        }
        self.slides.swap(ix, other);
        if self.config.active == ix {
            self.config.active = other;
        } else if self.config.active == other {
            self.config.active = ix;
        }
        self.snap(cx);
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The dock never sees a hosted panel, so the slides offer this host
        // from their own menus.
        let slide_title = rox_i18n::t!("slide-title");
        composite::report_hosted(
            self.slides.iter(),
            self.config.chrome.title.as_deref().unwrap_or(&slide_title),
            cx,
        );
        let active = self.config.active;
        let count = self.slides.len();
        let root = div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette::bg_root())
            .track_focus(&self.focus);

        if count == 0 {
            let weak = cx.entity().downgrade();
            let empty = composite::empty_slot(
                "slide-add-first",
                self.state.clone(),
                self.workspace.clone(),
                move |panel, window, cx| {
                    if let Some(this) = weak.upgrade() {
                        this.update(cx, |this, cx| this.add(panel, window, cx));
                    }
                },
            );
            if self.config.chrome.controls_hidden() {
                return root.child(empty);
            }
            let parent = composite::parent_button(rox_i18n::t!("slide-title"), cx);
            return root
                .child(empty)
                .child(composite::parent_controls().child(parent));
        }

        let pos = self.pos();
        if self.drag.is_none() && (pos - active as f32).abs() > f32::EPSILON {
            window.request_animation_frame();
        }

        let strip = self.slides.iter().enumerate().filter_map(|(i, child)| {
            let offset = i as f32 - pos;
            if offset.abs() >= 1.0 {
                return None;
            }
            Some(
                div()
                    .absolute()
                    .top_0()
                    .left(relative(offset))
                    .size_full()
                    .overflow_hidden()
                    .child(child.view()),
            )
        });
        let root = root.children(strip);

        let root = root.when(active > 0, |d| {
            let weak = cx.entity().downgrade();
            d.child(edge_scrim(false)).child(
                div()
                    .absolute()
                    .left(tokens::SPACE_XS)
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        Button::new("slide-prev")
                            .icon(Icon::default().path(icons::CHEVRON_LEFT))
                            .small()
                            .ghost()
                            .on_click(move |_, window, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        let target = this.config.active.saturating_sub(1);
                                        this.go(target, window, cx);
                                    });
                                }
                            }),
                    ),
            )
        });
        let root = root.when(active + 1 < count, |d| {
            let weak = cx.entity().downgrade();
            d.child(edge_scrim(true)).child(
                div()
                    .absolute()
                    .right(tokens::SPACE_XS)
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(
                        Button::new("slide-next")
                            .icon(Icon::default().path(icons::CHEVRON_RIGHT))
                            .small()
                            .ghost()
                            .on_click(move |_, window, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        let target = this.config.active + 1;
                                        this.go(target, window, cx);
                                    });
                                }
                            }),
                    ),
            )
        });

        // The canvas behind the dots keeps the rail's bounds fresh and re-arms
        // the window-level drag handlers each paint.
        let root = root.when(count > 1, |d| {
            let weak = cx.entity().downgrade();
            let dragging = self.rail.is_dragging();
            d.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(RAIL_H)
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(tokens::SPACE_XS)
                    .cursor_grab()
                    .when(dragging, |d| d.cursor_grabbing())
                    .hover(|d| d.bg(palette::alpha(palette::bg_control(), 0x30)))
                    // Down or right steps forward, up or left back.
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let lines = match event.delta {
                            ScrollDelta::Lines(lines) => lines.y + lines.x,
                            ScrollDelta::Pixels(pixels) => f32::from(pixels.y + pixels.x) / 20.0,
                        };
                        this.wheel += lines;
                        // A wheel notch arrives as 3 lines.
                        while this.wheel <= -3.0 {
                            this.wheel += 3.0;
                            let target = this.config.active + 1;
                            this.go(target, window, cx);
                        }
                        while this.wheel >= 3.0 {
                            this.wheel -= 3.0;
                            let target = this.config.active.saturating_sub(1);
                            this.go(target, window, cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_rail_drag(event.position.x, cx);
                        }),
                    )
                    .child(
                        canvas(
                            {
                                let rail = self.rail.clone();
                                move |bounds, _, _| rail.set_bounds(bounds)
                            },
                            {
                                let rail = self.rail.clone();
                                let weak = weak.clone();
                                move |_, _, window, _| {
                                    rail_on_paint(&rail, &weak, window);
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .children((0..count).map(move |i| {
                        let weak = weak.clone();
                        div()
                            .size(px(8.))
                            .rounded_full()
                            .cursor_pointer()
                            .bg(if i == active {
                                palette::accent()
                            } else {
                                palette::bg_control()
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_: &MouseDownEvent, window, cx| {
                                    if let Some(this) = weak.upgrade() {
                                        this.update(cx, |this, cx| this.go(i, window, cx));
                                    }
                                },
                            )
                    })),
            )
        });

        // Finished layouts hide the builder's buttons; the Workspace page's
        // tree still manages slides. The rail and dots stay.
        if self.config.chrome.controls_hidden() {
            return root;
        }

        let add_weak = cx.entity().downgrade();
        let controls = composite::corner_controls()
            .child(
                Button::new("slide-add")
                    .icon(Icon::default().path(icons::PLUS))
                    .small()
                    .ghost()
                    .tooltip(rox_i18n::t!("slide-add"))
                    .dropdown_menu({
                        let state = self.state.clone();
                        let workspace = self.workspace.clone();
                        move |menu, window, cx| {
                            let add_weak = add_weak.clone();
                            composite::pick_items(
                                menu,
                                state.clone(),
                                workspace.clone(),
                                window,
                                cx,
                                move |panel, window, cx| {
                                    if let Some(this) = add_weak.upgrade() {
                                        this.update(cx, |this, cx| this.add(panel, window, cx));
                                    }
                                },
                            )
                        }
                    }),
            )
            .children(self.slides.get(active).cloned().map(|child| {
                composite::slot_button(
                    ("slide-slot", active),
                    child,
                    self.state.clone(),
                    self.workspace.clone(),
                    move |this: &mut Self, panel, cx| this.replace(active, panel, cx),
                    move |this: &mut Self, cx| this.remove(active, cx),
                    move |menu, weak| {
                        let left = weak.clone();
                        let right = weak;
                        menu.item(
                            PopupMenuItem::new(rox_i18n::t!("composite-move-left"))
                                .icon(Icon::default().path(icons::CHEVRON_LEFT))
                                .disabled(active == 0)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = left.upgrade() {
                                        this.update(cx, |this, cx| this.shift(active, false, cx));
                                    }
                                }),
                        )
                        .item(
                            PopupMenuItem::new(rox_i18n::t!("composite-move-right"))
                                .icon(Icon::default().path(icons::CHEVRON_RIGHT))
                                .disabled(active + 1 >= count)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = right.upgrade() {
                                        this.update(cx, |this, cx| this.shift(active, true, cx));
                                    }
                                }),
                        )
                        .separator()
                    },
                    cx,
                )
            }));
        let parent = composite::parent_button(rox_i18n::t!("slide-title"), cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

fn edge_scrim(right: bool) -> Div {
    let shade = |alpha: u8| {
        let edge = linear_color_stop(palette::alpha(palette::bg_root_opaque(), alpha), 0.0);
        let fade = linear_color_stop(palette::alpha(palette::bg_root_opaque(), 0x00), 1.0);
        // Angle 90 runs 0% at the left, so the right scrim flips the stops.
        if right {
            linear_gradient(90., fade, edge)
        } else {
            linear_gradient(90., edge, fade)
        }
    };
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(SCRIM_W)
        .map(|d| if right { d.right_0() } else { d.left_0() })
        .bg(shade(0x38))
        .hover(move |d| d.bg(shade(0x78)))
}

/// Called from the rail's paint pass, since window handlers only last one
/// frame.
fn rail_on_paint(rail: &ScrubState, weak: &WeakEntity<SlidePanel>, window: &mut Window) {
    if !rail.is_dragging() {
        return;
    }
    window.on_mouse_event({
        let rail = rail.clone();
        let weak = weak.clone();
        move |event: &MouseMoveEvent, phase, window, cx| {
            if !phase.bubble() || !rail.is_dragging() {
                return;
            }
            let Some(this) = weak.upgrade() else {
                return;
            };
            // A release outside the window never reaches the up handler, so a
            // move without the button held ends the drag.
            if event.pressed_button != Some(MouseButton::Left) {
                this.update(cx, |this, cx| this.end_rail_drag(window, cx));
                return;
            }
            if let Some(frac) = rail.fraction(event.position.x) {
                this.update(cx, |this, cx| this.rail_drag_to(frac, cx));
            }
        }
    });
    window.on_mouse_event({
        let rail = rail.clone();
        let weak = weak.clone();
        move |_: &MouseUpEvent, phase, window, cx| {
            if !phase.bubble() || !rail.is_dragging() {
                return;
            }
            if let Some(this) = weak.upgrade() {
                this.update(cx, |this, cx| this.end_rail_drag(window, cx));
            }
        }
    });
}

impl PanelSettings for SlidePanel {
    fn composite(&self) -> bool {
        true
    }

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
}

impl EventEmitter<PanelEvent> for SlidePanel {}

impl Focusable for SlidePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for SlidePanel {
    fn panel_name(&self) -> &'static str {
        "slide"
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let slots: Vec<composite::Slot> = self.slides.iter().cloned().map(Some).collect();
        composite::open_slot_settings(&slots, window, cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("slide-title"),
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

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(child) = self.slides.get(self.config.active) {
            child.set_active(active, window, cx);
        }
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        let slots: Vec<Slot> = self.slides.iter().cloned().map(Some).collect();
        state.children = composite::dump_slots(&slots, cx);
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.introduced = false;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.introduced = false;
        for child in self.slides.iter() {
            child.on_removed(window, cx);
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let prev = cx.entity().downgrade();
        let next = cx.entity().downgrade();
        let menu = menu
            .item(
                PopupMenuItem::new(rox_i18n::t!("slide-previous"))
                    .icon(Icon::default().path(icons::CHEVRON_LEFT))
                    .disabled(self.config.active == 0)
                    .on_click(move |_, window, cx| {
                        if let Some(this) = prev.upgrade() {
                            this.update(cx, |this, cx| {
                                let target = this.config.active.saturating_sub(1);
                                this.go(target, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("slide-next"))
                    .icon(Icon::default().path(icons::CHEVRON_RIGHT))
                    .disabled(self.config.active + 1 >= self.slides.len())
                    .on_click(move |_, window, cx| {
                        if let Some(this) = next.upgrade() {
                            this.update(cx, |this, cx| {
                                let target = this.config.active + 1;
                                this.go(target, window, cx);
                            });
                        }
                    }),
            );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for SlidePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composite::introduce_slots(
            self.slides.iter(),
            &self.tab_panel,
            &mut self.introduced,
            window,
            cx,
        );
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
