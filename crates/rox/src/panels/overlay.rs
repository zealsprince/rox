//! The overlay panel: a main panel with a second layered over it as a card,
//! revealed by a corner button or Tab with a short fade. The main stays visible
//! under a scrim. Hosted through [`crate::composite`].

use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, Pixels, SharedString, WeakEntity, Window, div, prelude::*,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::composite::{self, Slot};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{ScrubState, setting_row};

const OVERLAY_INSET: Pixels = tokens::SPACE_MD;
const DEFAULT_DIM: f32 = 150.0 / 255.0;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub revealed: bool,
    pub dim: f32,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        OverlayConfig {
            chrome: PanelChrome::default(),
            revealed: false,
            dim: DEFAULT_DIM,
        }
    }
}

pub struct OverlayPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: OverlayConfig,
    /// Main at 0, overlay at 1.
    slots: [Slot; 2],
    active: bool,
    fade_at: Instant,
    dim_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    introduced: bool,
}

impl OverlayPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: OverlayConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: OverlayConfig,
        slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut fixed: [Slot; 2] = [None, None];
        for (slot, restored) in fixed.iter_mut().zip(slots) {
            *slot = restored;
        }
        OverlayPanel {
            state,
            workspace,
            config,
            slots: fixed,
            active: false,
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            dim_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            introduced: false,
        }
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    fn shown_ix(&self) -> usize {
        usize::from(self.config.revealed)
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.revealed = !self.config.revealed;
        // The main never leaves view, so it stays active; the overlay only
        // while up.
        if let Some(overlay) = &self.slots[1] {
            overlay.set_active(self.active && self.config.revealed, window, cx);
        }
        self.fade_at = Instant::now();
        cx.notify();
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        self.introduced = false;
        cx.notify();
    }

    fn set_dim(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.dim = fraction;
        cx.notify();
    }

    fn slot_content(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        match &self.slots[ix] {
            Some(child) => div().size_full().child(child.view()),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    if ix == 0 {
                        "overlay-add-0"
                    } else {
                        "overlay-add-1"
                    },
                    self.state.clone(),
                    self.workspace.clone(),
                    move |panel, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.set_slot(ix, Some(panel), cx));
                        }
                    },
                )
            }
        }
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // The dock never sees a hosted panel, so the children offer this host
        // from their own menus.
        let overlay_title = rox_i18n::t!("overlay-title");
        composite::report_hosted(
            self.slots.iter().flatten(),
            self.config
                .chrome
                .title
                .as_deref()
                .unwrap_or(&overlay_title),
            cx,
        );

        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        }
        let u = u * u * (3.0 - 2.0 * u);
        let overlay_alpha = if self.config.revealed { u } else { 1.0 - u };

        let root = div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // Capture phase, so Tab flips the overlay ahead of the focused
            // child.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.key != "tab"
                    || key.modifiers.control
                    || key.modifiers.alt
                    || key.modifiers.platform
                {
                    return;
                }
                if !this.focus.contains_focused(window, cx) {
                    return;
                }
                cx.stop_propagation();
                this.toggle(window, cx);
            }))
            // Keys only dispatch along the focus path, so a click landing
            // nowhere focusable pulls focus to the panel for Tab to work.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus);
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .overflow_hidden()
                    .child(self.slot_content(0, cx)),
            );

        let root = if overlay_alpha > 0.001 {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(overlay_alpha)
                    .child(div().absolute().inset_0().bg(palette::alpha(
                        palette::bg_root_opaque(),
                        (self.config.dim.clamp(0.0, 1.0) * 255.0).round() as u8,
                    )))
                    // Occluded so clicks never fall through to the dimmed main.
                    // The card only fills while the slot is empty; a hosted
                    // panel's own surface is the background.
                    .child(
                        div().absolute().inset_0().p(OVERLAY_INSET).child(
                            div()
                                .size_full()
                                .rounded(tokens::RADIUS)
                                .overflow_hidden()
                                .border_1()
                                .border_color(palette::border())
                                .when(self.slots[1].is_none(), |d| d.bg(palette::bg_root()))
                                .shadow_md()
                                .occlude()
                                .child(self.slot_content(1, cx)),
                        ),
                    ),
            )
        } else {
            root
        };

        // Finished layouts hide the builder's buttons; the Workspace page's
        // tree still swaps slots.
        if self.config.chrome.controls_hidden() {
            return root;
        }

        let shown = self.shown_ix();
        let toggle = cx.entity().downgrade();
        let controls = composite::corner_controls()
            .child(
                Button::new("overlay-toggle")
                    .icon(Icon::default().path(icons::LAYERS))
                    .small()
                    .ghost()
                    .tooltip(rox_i18n::t!("overlay-toggle"))
                    .on_click(move |_, window, cx| {
                        if let Some(this) = toggle.upgrade() {
                            this.update(cx, |this, cx| this.toggle(window, cx));
                        }
                    }),
            )
            .children(self.slots[shown].clone().map(|child| {
                composite::slot_button(
                    ("overlay-slot", shown),
                    child,
                    self.state.clone(),
                    self.workspace.clone(),
                    move |this: &mut Self, panel, cx| this.set_slot(shown, Some(panel), cx),
                    move |this: &mut Self, cx| this.set_slot(shown, None, cx),
                    |menu, _| menu,
                    cx,
                )
            }));
        let parent = composite::parent_button(rox_i18n::t!("overlay-title"), cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

impl PanelSettings for OverlayPanel {
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dim = self.config.dim.clamp(0.0, 1.0);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(setting_row(
                    rox_i18n::t!("overlay-dim"),
                    Some(rox_i18n::t!("overlay-dim.description")),
                    settings_ui::scalar(
                        &self.dim_scrub,
                        &self.value_edit,
                        dim * 100.0,
                        settings_ui::span(0., 100., "%").hard(),
                        |this: &mut Self, percent, cx| this.set_dim(percent / 100.0, cx),
                        cx,
                    ),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for OverlayPanel {}

impl Focusable for OverlayPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for OverlayPanel {
    fn panel_name(&self) -> &'static str {
        "overlay"
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        composite::open_slot_settings(&self.slots, window, cx);
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("overlay-title"),
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
        self.active = active;
        if let Some(main) = &self.slots[0] {
            main.set_active(active, window, cx);
        }
        if let Some(overlay) = &self.slots[1] {
            overlay.set_active(active && self.config.revealed, window, cx);
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
        state.children = composite::dump_slots(&self.slots, cx);
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
        for child in self.slots.iter().flatten() {
            child.on_removed(window, cx);
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let toggle = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("overlay-toggle"))
                .icon(Icon::default().path(icons::LAYERS))
                .on_click(move |_, window, cx| {
                    if let Some(this) = toggle.upgrade() {
                        this.update(cx, |this, cx| this.toggle(window, cx));
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

impl Render for OverlayPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composite::introduce_slots(
            self.slots.iter().flatten(),
            &self.tab_panel,
            &mut self.introduced,
            window,
            cx,
        );
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
