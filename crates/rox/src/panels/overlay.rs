//! The overlay panel: a main panel with a second panel layered over it,
//! a corner button (or Tab) revealing the overlay with a short fade. The
//! main stays visible below, dimmed under a scrim, so the overlay reads
//! as a modal card floating on top instead of a full swap. For the pairs
//! that share one spot but want both in view (a library with its stats
//! over it, cover art with lyrics on top). Hosted through
//! [`crate::composite`]; the overlay costs nothing once it settles hidden.

use std::time::Instant;

use gpui::{
    div, prelude::*, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, Pixels, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::composite::{self, Slot};
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::workspace::Workspace;

/// The margin the revealed overlay leaves around itself, so the main panel
/// frames it on every side.
const OVERLAY_INSET: Pixels = tokens::SPACE_MD;
/// How hard the main panel dims under a fully revealed overlay by
/// default, the dim setting's starting point.
const DEFAULT_DIM: f32 = 150.0 / 255.0;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Whether the overlay is up: false shows the main alone, true layers
    /// the overlay on top of it.
    pub revealed: bool,
    /// How hard the scrim dims the main under the revealed overlay, 0
    /// leaving it bare to 1 blacking it out.
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
    /// Whether the panel itself is active, so a toggle can hand the overlay
    /// the right active state without waiting on the next dock call.
    active: bool,
    /// When the last toggle started; a restore lands settled.
    fade_at: Instant,
    /// The settings page's dim slider strip.
    dim_scrub: ScrubState,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
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

    /// Build with already-restored children, the layout-dump route in.
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
            focus: cx.focus_handle(),
            tab_panel: None,
        }
    }

    /// The hosted slots, main then overlay, for the settings window's
    /// layout tree.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// The slot the corner controls act on: the overlay while it is up, the
    /// main otherwise.
    fn shown_ix(&self) -> usize {
        usize::from(self.config.revealed)
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.revealed = !self.config.revealed;
        // The overlay runs only while it is up; the main below keeps running
        // the whole time, since it never leaves view.
        if let Some(overlay) = &self.slots[1] {
            overlay.set_active(self.active && self.config.revealed, window, cx);
        }
        self.fade_at = Instant::now();
        cx.notify();
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        cx.notify();
    }

    fn set_dim(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.config.dim = fraction;
        cx.notify();
    }

    /// One slot's content: the child's view or the empty add affordance,
    /// filling whatever box wraps it.
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
        // Frames only while a toggle is actually running; a settled panel
        // costs zero.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);
        // The overlay fades in as it reveals, out as it hides. The main
        // below never moves.
        let overlay_alpha = if self.config.revealed { u } else { 1.0 - u };

        let root = div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // Tab flips the overlay from anywhere inside the panel, ahead of
            // whatever the focused child would do with the key.
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
            // Keys only dispatch along the focus path, so Tab needs focus
            // somewhere inside the panel. Children that take focus claim it
            // first (bubble order); a click landing nowhere focusable pulls
            // focus to the panel itself.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus);
                    }
                }),
            )
            // The main panel is always the base, at full weight.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .overflow_hidden()
                    .child(self.slot_content(0, cx)),
            );

        // The overlay layer, scrim and floating card together, only while it
        // is up or still fading.
        let root = if overlay_alpha > 0.001 {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(overlay_alpha)
                    // The scrim dims the main so the overlay reads as being
                    // in front, not beside.
                    .child(div().absolute().inset_0().bg(palette::alpha(
                        palette::bg_root_opaque(),
                        (self.config.dim.clamp(0.0, 1.0) * 255.0).round() as u8,
                    )))
                    // The overlay itself, inset so the main frames it.
                    // Occluded so clicks on the card never fall through to
                    // the dimmed main beneath it. The card only fills while
                    // the slot is empty; a hosted panel's surface is the
                    // background, so its opacity override (the Appearance
                    // page) decides how much of the main shows through.
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

        let shown = self.shown_ix();
        let toggle = cx.entity().downgrade();
        let controls = composite::corner_controls()
            .child(
                Button::new("overlay-toggle")
                    .icon(Icon::default().path(icons::LAYERS))
                    .small()
                    .ghost()
                    .tooltip("Toggle overlay")
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
        let parent = composite::parent_button("Overlay", cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

impl PanelSettings for OverlayPanel {
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
                    "Dim",
                    Some("How hard the main panel dims under the revealed overlay"),
                    panel::value_slider(
                        &self.dim_scrub,
                        dim,
                        format!("{:.0} %", dim * 100.0),
                        Self::set_dim,
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Overlay")
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
        // The main tracks the panel; the overlay tracks it only while up.
        if let Some(main) = &self.slots[0] {
            main.set_active(active, window, cx);
        }
        if let Some(overlay) = &self.slots[1] {
            overlay.set_active(active && self.config.revealed, window, cx);
        }
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
        let toggle = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Toggle overlay")
                .icon(Icon::default().path(icons::LAYERS))
                .on_click(move |_, window, cx| {
                    if let Some(this) = toggle.upgrade() {
                        this.update(cx, |this, cx| this.toggle(window, cx));
                    }
                }),
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for OverlayPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
