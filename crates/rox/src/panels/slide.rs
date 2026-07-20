//! The slide panel: a carousel of panels in one slot, one up at a time,
//! arrows and dots gliding between them. For the surfaces that take
//! turns rather than share space - visualizers to cycle through, a set
//! of library views on rotation. Hosted through [`crate::composite`];
//! only the slides touching the viewport render, so a long deck costs
//! what a single panel does.

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    div, prelude::*, px, relative, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, PanelView, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::composite::{self, Slot};
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::workspace::Workspace;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlideConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The slide showing (or being glid toward).
    pub active: usize,
}

pub struct SlidePanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: SlideConfig,
    slides: Vec<Arc<dyn PanelView>>,
    /// Where the glide started from, in slide positions; with
    /// `slide_at` this gives the animated position without per-frame
    /// state.
    from: f32,
    slide_at: Instant,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
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

    /// Build with already-restored children, the layout-dump route in.
    /// Slides have no holes, so empty sentinels (a hand-edited dump)
    /// drop out; the active index re-clamps against what actually came
    /// back.
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
            focus: cx.focus_handle(),
            tab_panel: None,
        }
    }

    /// The deck in slide order, for the settings window's layout tree.
    pub fn slides(&self) -> &[Arc<dyn PanelView>] {
        &self.slides
    }

    /// The animated position in slide units: eased from `from` toward
    /// the active index, settled once the glide's window passes.
    fn pos(&self) -> f32 {
        let u = (self.slide_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        let u = u * u * (3.0 - 2.0 * u);
        self.from + (self.config.active as f32 - self.from) * u
    }

    /// Glide to `target`; out-of-range asks clamp, so the arrows never
    /// need their own guards.
    fn go(&mut self, target: usize, cx: &mut Context<Self>) {
        let target = target.min(self.slides.len().saturating_sub(1));
        if target == self.config.active {
            return;
        }
        self.from = self.pos();
        self.config.active = target;
        self.slide_at = Instant::now();
        cx.notify();
    }

    /// Pin the position to the active slide with no glide, for the edits
    /// that reorder the deck under it.
    fn snap(&mut self, cx: &mut Context<Self>) {
        self.from = self.config.active as f32;
        self.slide_at = Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS);
        cx.notify();
    }

    fn add(&mut self, panel: Arc<dyn PanelView>, cx: &mut Context<Self>) {
        self.slides.push(panel);
        if self.slides.len() == 1 {
            self.snap(cx);
        } else {
            self.go(self.slides.len() - 1, cx);
        }
    }

    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.slides.len() {
            return;
        }
        self.slides.remove(ix);
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

    /// Move slide `ix` one step left or right, following it with the
    /// view when it was the active one.
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
                move |panel, _, cx| {
                    if let Some(this) = weak.upgrade() {
                        this.update(cx, |this, cx| this.add(panel, cx));
                    }
                },
            );
            let parent = composite::parent_button("Slide", cx);
            return root
                .child(empty)
                .child(composite::parent_controls().child(parent));
        }

        // Frames only while a glide is actually running.
        let pos = self.pos();
        if (pos - active as f32).abs() > f32::EPSILON {
            window.request_animation_frame();
        }

        // Only the slides touching the viewport mount; the rest of the
        // deck stays idle entities.
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

        // The edge arrows, only where a neighbor exists. Bare containers
        // don't catch clicks, so the full-height wrapper never blocks the
        // slide under it - only the button does.
        let root = root.when(active > 0, |d| {
            let weak = cx.entity().downgrade();
            d.child(
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
                            .on_click(move |_, _, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        let target = this.config.active.saturating_sub(1);
                                        this.go(target, cx);
                                    });
                                }
                            }),
                    ),
            )
        });
        let root = root.when(active + 1 < count, |d| {
            let weak = cx.entity().downgrade();
            d.child(
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
                            .on_click(move |_, _, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        let target = this.config.active + 1;
                                        this.go(target, cx);
                                    });
                                }
                            }),
                    ),
            )
        });

        // The dots, once there is something to move between.
        let root = root.when(count > 1, |d| {
            let weak = cx.entity().downgrade();
            d.child(
                div()
                    .absolute()
                    .bottom(tokens::SPACE_SM)
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .gap(tokens::SPACE_XS)
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
                            .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, _, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| this.go(i, cx));
                                }
                            })
                    })),
            )
        });

        // The corner controls: add a slide, and the active slide's menu
        // with its reorder moves ahead of the shared rows.
        let add_weak = cx.entity().downgrade();
        let controls = composite::corner_controls()
            .child(
                Button::new("slide-add")
                    .icon(Icon::default().path(icons::PLUS))
                    .small()
                    .ghost()
                    .tooltip("Add Slide")
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
                                move |panel, _, cx| {
                                    if let Some(this) = add_weak.upgrade() {
                                        this.update(cx, |this, cx| this.add(panel, cx));
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
                            PopupMenuItem::new("Move Left")
                                .icon(Icon::default().path(icons::CHEVRON_LEFT))
                                .disabled(active == 0)
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = left.upgrade() {
                                        this.update(cx, |this, cx| this.shift(active, false, cx));
                                    }
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Move Right")
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
        let parent = composite::parent_button("Slide", cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

impl PanelSettings for SlidePanel {
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Slide")
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
        let prev = cx.entity().downgrade();
        let next = cx.entity().downgrade();
        let menu = menu
            .item(
                PopupMenuItem::new("Previous Slide")
                    .icon(Icon::default().path(icons::CHEVRON_LEFT))
                    .disabled(self.config.active == 0)
                    .on_click(move |_, _, cx| {
                        if let Some(this) = prev.upgrade() {
                            this.update(cx, |this, cx| {
                                let target = this.config.active.saturating_sub(1);
                                this.go(target, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("Next Slide")
                    .icon(Icon::default().path(icons::CHEVRON_RIGHT))
                    .disabled(self.config.active + 1 >= self.slides.len())
                    .on_click(move |_, _, cx| {
                        if let Some(this) = next.upgrade() {
                            this.update(cx, |this, cx| {
                                let target = this.config.active + 1;
                                this.go(target, cx);
                            });
                        }
                    }),
            );
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for SlidePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
