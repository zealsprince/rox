//! The depth panel: two panels in one slot, front over back, a corner
//! button flipping between them with a short cross-fade - the cover
//! panel's slide idiom over hosted panels instead of art. For the pairs
//! that trade the same spot (a cover art with lyrics behind it, a
//! library with its stats). Hosted through [`crate::composite`]; the
//! hidden side costs nothing once the fade settles.

use std::time::Instant;

use gpui::{
    div, prelude::*, App, Context, Div, EventEmitter, FocusHandle, Focusable, SharedString,
    WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::composite::{self, Slot};
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::workspace::Workspace;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DepthConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Which side is up: false shows the front slot, true the back.
    pub revealed: bool,
}

pub struct DepthPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: DepthConfig,
    /// Front at 0, back at 1.
    slots: [Slot; 2],
    /// When the last flip started; a restore lands settled.
    fade_at: Instant,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
}

impl DepthPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: DepthConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    /// Build with already-restored children, the layout-dump route in.
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: DepthConfig,
        slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut fixed: [Slot; 2] = [None, None];
        for (slot, restored) in fixed.iter_mut().zip(slots) {
            *slot = restored;
        }
        DepthPanel {
            state,
            workspace,
            config,
            slots: fixed,
            fade_at: Instant::now() - std::time::Duration::from_secs_f32(tokens::EASE_SECS),
            focus: cx.focus_handle(),
            tab_panel: None,
        }
    }

    /// The hosted slots, front then back, for the settings window's
    /// layout tree.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// The slot currently up (or fading in).
    fn shown_ix(&self) -> usize {
        usize::from(self.config.revealed)
    }

    fn flip(&mut self, cx: &mut Context<Self>) {
        self.config.revealed = !self.config.revealed;
        self.fade_at = Instant::now();
        cx.notify();
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        cx.notify();
    }

    /// One side's content at a weight: the child's view or the empty add
    /// affordance, filling the panel. Opacity cascades, so the whole side
    /// fades as one.
    fn layer(&self, ix: usize, opacity: f32, cx: &mut Context<Self>) -> Div {
        let content = match &self.slots[ix] {
            Some(child) => div().size_full().child(child.view()),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    if ix == 0 { "depth-add-0" } else { "depth-add-1" },
                    self.state.clone(),
                    self.workspace.clone(),
                    move |panel, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.set_slot(ix, Some(panel), cx));
                        }
                    },
                )
            }
        };
        div()
            .absolute()
            .inset_0()
            .opacity(opacity)
            .overflow_hidden()
            .child(content)
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Frames only while a flip is actually running; a settled panel
        // costs zero.
        let u = (self.fade_at.elapsed().as_secs_f32() / tokens::EASE_SECS).min(1.0);
        if u < 1.0 {
            window.request_animation_frame();
        }
        // Smoothstepped so the fade eases out instead of stopping dead.
        let u = u * u * (3.0 - 2.0 * u);

        let shown = self.shown_ix();
        let root = div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus);
        // The outgoing side holds full under the incoming one, so the
        // flip never dips toward the backdrop mid-fade.
        let root = if u >= 1.0 {
            root.child(self.layer(shown, 1.0, cx))
        } else {
            root.child(self.layer(1 - shown, 1.0, cx))
                .child(self.layer(shown, u, cx))
        };

        let flip = cx.entity().downgrade();
        let controls = composite::corner_controls()
            .child(
                Button::new("depth-flip")
                    .icon(Icon::default().path(icons::LAYERS))
                    .small()
                    .ghost()
                    .tooltip("Flip")
                    .on_click(move |_, _, cx| {
                        if let Some(this) = flip.upgrade() {
                            this.update(cx, |this, cx| this.flip(cx));
                        }
                    }),
            )
            .children(self.slots[shown].clone().map(|child| {
                composite::slot_button(
                    ("depth-slot", shown),
                    child,
                    self.state.clone(),
                    self.workspace.clone(),
                    move |this: &mut Self, panel, cx| this.set_slot(shown, Some(panel), cx),
                    move |this: &mut Self, cx| this.set_slot(shown, None, cx),
                    |menu, _| menu,
                    cx,
                )
            }));
        let parent = composite::parent_button("Depth", cx);
        root.child(controls)
            .child(composite::parent_controls().child(parent))
    }
}

impl PanelSettings for DepthPanel {
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

impl EventEmitter<PanelEvent> for DepthPanel {}

impl Focusable for DepthPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for DepthPanel {
    fn panel_name(&self) -> &'static str {
        "depth"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Depth")
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
        if let Some(child) = &self.slots[self.shown_ix()] {
            child.set_active(active, window, cx);
        }
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
        let flip = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Flip")
                .icon(Icon::default().path(icons::LAYERS))
                .on_click(move |_, _, cx| {
                    if let Some(this) = flip.upgrade() {
                        this.update(cx, |this, cx| this.flip(cx));
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

impl Render for DepthPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}
