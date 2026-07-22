use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crate::PanelInfo;
use crate::tab::{Tab, TabBar};
use gpui::{
    App, AppContext, Bounds, Context, Corner, DismissEvent, Div, DragMoveEvent, Empty, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    ScrollHandle, SharedString, Size, StatefulInteractiveElement, StyleRefinement, Styled,
    Subscription, WeakEntity, Window, anchored, canvas, deferred, div, prelude::FluentBuilder, px,
    relative, rems,
};
use gpui_component::{
    ActiveTheme, AxisExt, IconName, Placement, Selectable, Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu, PopupMenu, PopupMenuItem},
    v_flex,
};

use super::{
    ClosePanel, DockArea, DockPlacement, Panel, PanelControl, PanelEvent, PanelState, PanelStyle,
    PanelView, StackPanel, ToggleZoom,
};

#[derive(Clone)]
struct TabState {
    zoomable: Option<PanelControl>,
    draggable: bool,
    droppable: bool,
    active_panel: Option<Arc<dyn PanelView>>,
}

/// A panel move riding the middle mouse button, or Alt+Left where gpui's
/// built-in drag isn't listening: the hand-rolled twin of that built-in
/// left-button drag. The source TabPanel starts it past the drag threshold,
/// the DockArea tracks the pointer and hosts the chip, and the TabPanel
/// under the release finishes it through the same drop path as the
/// built-in drag.
pub(crate) struct MiddleDrag {
    pub(crate) panel: Arc<dyn PanelView>,
    pub(crate) source: WeakEntity<TabPanel>,
    /// The chip riding the pointer, prebuilt at drag start.
    pub(crate) chip: Entity<DragPanel>,
    pub(crate) position: Point<Pixels>,
    /// The button carrying the drag; only its release ends it. Middle, or
    /// Left when Alt was held at the press.
    pub(crate) button: MouseButton,
}

/// Movement below this is a middle click, past it a middle drag. Same value
/// as gpui's private DRAG_THRESHOLD.
const MIDDLE_DRAG_THRESHOLD: f64 = 2.;

#[derive(Clone)]
pub(crate) struct DragPanel {
    pub(crate) panel: Arc<dyn PanelView>,
    pub(crate) tab_panel: Entity<TabPanel>,
}

impl DragPanel {
    pub(crate) fn new(panel: Arc<dyn PanelView>, tab_panel: Entity<TabPanel>) -> Self {
        Self { panel, tab_panel }
    }
}

impl Render for DragPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("drag-panel")
            .cursor_grab()
            .py_1()
            .px_3()
            .w_24()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .text_color(cx.theme().tab_foreground)
            .bg(cx.theme().tab_active)
            .opacity(0.75)
            .child(self.panel.title(window, cx))
    }
}

pub struct TabPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    /// The stock_panel can be None, if is None, that means the panels can't be split or move
    stack_panel: Option<WeakEntity<StackPanel>>,
    pub(crate) panels: Vec<Arc<dyn PanelView>>,
    pub(crate) active_ix: usize,
    /// If this is true, the Panel closable will follow the active panel's closable,
    /// otherwise this TabPanel will not able to close
    ///
    /// This is used for Dock to limit the last TabPanel not able to close, see [`super::Dock::new`].
    pub(crate) closable: bool,

    tab_bar_scroll_handle: ScrollHandle,
    zoomed: bool,
    collapsed: bool,
    /// When drag move, will get the placement of the panel to be split
    will_split_placement: Option<Placement>,
    /// Is TabPanel used in Tiles.
    in_tiles: bool,
    /// The open right-click menu: its anchor position, the menu, and the
    /// dismiss subscription that clears it.
    context_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Where an arming button (middle, or left with Alt held) went down and
    /// on which panel - a tab arms its own panel, the body arms the active
    /// one. Becomes a [`MiddleDrag`] once the pointer moves past the
    /// threshold; released in place on a tab, a middle press stays a
    /// middle-click close.
    pending_middle_drag: Option<(Point<Pixels>, Arc<dyn PanelView>, MouseButton)>,
    /// The panel body's bounds, recorded at paint: mouse listeners don't
    /// carry element bounds the way DragMoveEvent does, and the middle-drag
    /// placement math needs them.
    content_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Panel for TabPanel {
    fn panel_name(&self) -> &'static str {
        "TabPanel"
    }

    fn title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.active_panel(cx)
            .map(|panel| panel.title(window, cx))
            .unwrap_or("Empty Tab".into_any_element())
    }

    fn closable(&self, cx: &App) -> bool {
        self.active_panel(cx)
            .map(|panel| self.can_close_panel(&panel, cx))
            .unwrap_or(false)
    }

    fn zoomable(&self, cx: &App) -> Option<PanelControl> {
        self.active_panel(cx).and_then(|panel| panel.zoomable(cx))
    }

    fn visible(&self, cx: &App) -> bool {
        self.visible_panels(cx).next().is_some()
    }

    fn min_size(&self, cx: &App) -> Size<Pixels> {
        // As demanding as the most demanding visible tab; any of them can
        // become the active one. Seed at zero, not the global floor, so a
        // panel that opts into a smaller min (the search bar) is honored;
        // panels keep the 40px floor by default through `Panel::min_size`.
        self.visible_panels(cx)
            .fold(gpui::size(px(0.), px(0.)), |acc, panel| {
                let min = panel.min_size(cx);
                gpui::size(acc.width.max(min.width), acc.height.max(min.height))
            })
    }

    fn max_size(&self, cx: &App) -> Size<Pixels> {
        // A tab shows one panel at a time in the same slot, so the most
        // permissive cap wins: a capped panel tabbed with an unbounded one
        // stays unbounded. No visible panel means no slot to cap.
        let mut any = false;
        let max = self
            .visible_panels(cx)
            .fold(gpui::size(px(0.), px(0.)), |acc, panel| {
                any = true;
                let m = panel.max_size(cx);
                gpui::size(acc.width.max(m.width), acc.height.max(m.height))
            });
        if any {
            max
        } else {
            gpui::size(Pixels::MAX, Pixels::MAX)
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        if let Some(panel) = self.active_panel(cx) {
            panel.dropdown_menu(menu, window, cx)
        } else {
            menu
        }
    }

    fn toolbar_buttons(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        self.active_panel(cx)
            .and_then(|panel| panel.toolbar_buttons(window, cx))
    }

    fn dump(&self, cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
        // Set the tabs info up front, not inside the loop: an empty tab panel
        // still has to dump as tabs. Left on the PanelState default of
        // `Panel(Null)`, restore routes "TabPanel" through the registry, which
        // has no builder for it, and the window comes back as an InvalidPanel.
        state.info = PanelInfo::tabs(self.active_ix);
        for panel in self.panels.iter() {
            state.add_child(panel.dump(cx));
        }
        state
    }

    fn inner_padding(&self, cx: &App) -> bool {
        self.active_panel(cx)
            .map_or(true, |panel| panel.inner_padding(cx))
    }
}

impl TabPanel {
    pub fn new(
        stack_panel: Option<WeakEntity<StackPanel>>,
        dock_area: WeakEntity<DockArea>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            stack_panel,
            panels: Vec::new(),
            active_ix: 0,
            tab_bar_scroll_handle: ScrollHandle::new(),
            will_split_placement: None,
            zoomed: false,
            collapsed: false,
            closable: true,
            in_tiles: false,
            context_menu: None,
            pending_middle_drag: None,
            content_bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    /// Mark the TabPanel as being used in Tiles.
    pub(super) fn set_in_tiles(&mut self, in_tiles: bool) {
        self.in_tiles = in_tiles;
    }

    pub(super) fn set_parent(&mut self, view: WeakEntity<StackPanel>) {
        self.stack_panel = Some(view);
    }

    /// Return current active_panel View
    pub fn active_panel(&self, cx: &App) -> Option<Arc<dyn PanelView>> {
        let panel = self.panels.get(self.active_ix);

        if let Some(panel) = panel {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                // Return the first visible panel
                self.visible_panels(cx).next()
            }
        } else {
            None
        }
    }

    /// Rox addition: make a specific panel the active tab and hand it the
    /// keyboard, whether or not it was already the active one. Used by the
    /// dock's focus-by-name so a shortcut can jump straight to a panel that
    /// sits behind other tabs. No-op if the panel isn't in this group.
    pub fn focus_panel(
        &mut self,
        panel: &Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.panels.iter().position(|p| p == panel) else {
            return false;
        };
        // set_active_ix focuses when the index changes; do it here too so an
        // already-active tab still takes focus.
        self.set_active_ix(ix, window, cx);
        panel.focus_handle(cx).focus(window);
        true
    }

    fn set_active_ix(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active_ix {
            return;
        }

        let last_active_ix = self.active_ix;

        self.active_ix = ix;
        self.tab_bar_scroll_handle.scroll_to_item(ix);
        self.focus_active_panel(window, cx);

        // Sync the active state to all panels
        cx.spawn_in(window, async move |view, cx| {
            _ = cx.update(|window, cx| {
                _ = view.update(cx, |view, cx| {
                    if let Some(last_active) = view.panels.get(last_active_ix) {
                        last_active.set_active(false, window, cx);
                    }
                    if let Some(active) = view.panels.get(view.active_ix) {
                        active.set_active(true, window, cx);
                    }
                });
            });
        })
        .detach();

        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add a panel to the end of the tabs
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_panel_with_active(panel, true, window, cx);
    }

    fn add_panel_with_active(
        &mut self,
        panel: Arc<dyn PanelView>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        assert_ne!(
            panel.panel_name(cx),
            "StackPanel",
            "can not allows add `StackPanel` to `TabPanel`"
        );

        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        panel.on_added_to(cx.entity().downgrade(), window, cx);
        self.panels.push(panel);
        // set the active panel to the new panel
        if active {
            self.set_active_ix(self.panels.len() - 1, window, cx);
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add panel to try to split
    pub fn add_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.will_split_placement = Some(placement);
                    view.split_panel(panel, placement, size, window, cx)
                })
                .ok()
            })
            .ok()
        })
        .detach();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn insert_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        panel.on_added_to(cx.entity().downgrade(), window, cx);
        self.panels.insert(ix, panel);
        self.set_active_ix(ix, window, cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Whether a specific panel in this group may close: the same guards as
    /// [`Panel::closable`], but for any panel rather than the active one.
    ///
    /// Deliberately looser than upstream, which also refused the last panel
    /// of a section: the workspace recovers an empty center through the
    /// View menu, so being last is no reason to stay.
    fn can_close_panel(&self, panel: &Arc<dyn PanelView>, cx: &App) -> bool {
        if !self.closable {
            return false;
        }

        // Locked groups (zoomed, or detached from any stack) keep their
        // panels - except in Tiles, where panels always may close.
        if self.is_locked(cx) && !self.in_tiles {
            return false;
        }

        panel.closable(cx)
    }

    /// Move a tab to another index, the app-level reorder route (the
    /// settings window's layout tree). The active index follows its
    /// panel, so the shown surface and focus never change. Out-of-range
    /// indices do nothing.
    pub fn move_panel(&mut self, from_ix: usize, to_ix: usize, cx: &mut Context<Self>) {
        if from_ix == to_ix || from_ix >= self.panels.len() || to_ix >= self.panels.len() {
            return;
        }
        let panel = self.panels.remove(from_ix);
        self.panels.insert(to_ix, panel);
        if self.active_ix == from_ix {
            self.active_ix = to_ix;
        } else if from_ix < self.active_ix && to_ix >= self.active_ix {
            self.active_ix -= 1;
        } else if from_ix > self.active_ix && to_ix <= self.active_ix {
            self.active_ix += 1;
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Lift the tab at `ix` out of this group into a group of its own,
    /// landing right after this one in the parent stack (the settings
    /// window's layout tree route). A solo tab already is its own group,
    /// so that and out-of-range indices do nothing.
    pub fn lift_panel(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.panels.len() <= 1 {
            return;
        }
        let Some(panel) = self.panels.get(ix).cloned() else {
            return;
        };
        let Some(stack) = self.stack_panel.as_ref().and_then(|stack| stack.upgrade()) else {
            return;
        };
        let this: Arc<dyn PanelView> = Arc::new(cx.entity().clone());
        let Some(stack_ix) = stack.read(cx).index_of_panel(this) else {
            return;
        };
        let dock_area = self.dock_area.clone();
        self.detach_panel(panel.clone(), window, cx);
        let new_tabs = cx.new(|cx| Self::new(None, dock_area.clone(), window, cx));
        new_tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
        stack.update(cx, |stack, cx| {
            stack.insert_panel_after(Arc::new(new_tabs), stack_ix, None, dock_area, window, cx);
        });
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Remove a panel from the tab panel
    pub fn remove_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detach_panel(panel, window, cx);
        self.remove_self_if_empty(window, cx);
        // Only unzoom when this group holds the zoom; an unconditional
        // ZoomOut would make DockArea drop another group's zoomed view.
        // The flag has to follow the event or the zoom menu label lags one
        // click behind after a pop-out.
        if self.zoomed {
            self.zoomed = false;
            cx.emit(PanelEvent::ZoomOut);
        }
        cx.emit(PanelEvent::LayoutChanged);
        // Wake observers - remaining panels watch the group to know when
        // they become solo.
        cx.notify();
    }

    fn detach_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        panel.on_removed(window, cx);
        let panel_view = panel.view();
        let removed_ix = self.panels.iter().position(|p| p.view() == panel_view);
        self.panels.retain(|p| p.view() != panel_view);
        // Removing a tab left of the active one shifts the vec, so the
        // active index has to shift with it or the shown tab jumps. The
        // active panel itself doesn't change, so no activation round-trip.
        if let Some(removed_ix) = removed_ix {
            if removed_ix < self.active_ix {
                self.active_ix -= 1;
            }
        }
        if self.active_ix >= self.panels.len() {
            self.set_active_ix(self.panels.len().saturating_sub(1), window, cx)
        }
    }

    /// Check to remove self from the parent StackPanel, if there is no panel left
    fn remove_self_if_empty(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panels.is_empty() {
            return;
        }

        let tab_view = cx.entity().clone();
        if let Some(stack_panel) = self.stack_panel.as_ref() {
            _ = stack_panel.update(cx, |view, cx| {
                view.remove_panel(Arc::new(tab_view), window, cx);
            });
        }
    }

    pub(super) fn set_collapsed(
        &mut self,
        collapsed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapsed = collapsed;
        if let Some(panel) = self.panels.get(self.active_ix) {
            panel.set_active(!collapsed, window, cx);
        }
        cx.notify();
    }

    fn is_locked(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return true;
        };

        if dock_area.read(cx).is_locked() {
            return true;
        }

        if self.zoomed {
            return true;
        }

        // A panel that asked to be pinned locks its group: no dragging it
        // out, no rearranging, nothing dropped in. rox builds one panel
        // per group, so this reads as per-panel; a mixed group locks as a
        // whole, the conservative call.
        if self.panels.iter().any(|panel| panel.locked(cx)) {
            return true;
        }

        self.stack_panel.is_none()
    }

    /// Return true if self or parent only have last panel.
    fn is_last_panel(&self, cx: &App) -> bool {
        if let Some(parent) = &self.stack_panel {
            if let Some(stack_panel) = parent.upgrade() {
                if !stack_panel.read(cx).is_last_panel(cx) {
                    return false;
                }
            }
        }

        self.panels.len() <= 1
    }

    /// Number of panels in the group. A panel can use this (with an observe
    /// on its hosting tab panel) to tell whether it is alone in its group -
    /// the dock draws no header then, so solo panels host their own
    /// controls. Deliberately the raw count, no visibility filter: checking
    /// visibility reads every panel entity, and a panel asking from inside
    /// its own render would deadlock on itself.
    pub fn panels_count(&self) -> usize {
        self.panels.len()
    }

    /// The tabs in order, for app-level walks over the live layout, the
    /// same entities `dump` serializes.
    pub fn panels(&self) -> &[Arc<dyn PanelView>] {
        &self.panels
    }

    /// The active tab's index.
    pub fn active_index(&self) -> usize {
        self.active_ix
    }

    /// Return all visible panels
    fn visible_panels<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = Arc<dyn PanelView>> + 'a {
        self.panels.iter().filter_map(|panel| {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                None
            }
        })
    }

    /// Return true if the tab panel is draggable.
    ///
    /// E.g. if the parent and self only have one panel, it is not draggable.
    fn draggable(&self, cx: &App) -> bool {
        !self.is_locked(cx) && !self.is_last_panel(cx)
    }

    /// Return true if the tab panel is droppable.
    ///
    /// E.g. if the tab panel is locked, it is not droppable.
    fn droppable(&self, cx: &App) -> bool {
        !self.is_locked(cx)
    }

    /// Open the right-click menu for a panel: the panel's own dropdown
    /// items (which carry their own close), then zoom, all acting on that
    /// panel rather than the active one. One menu per tab panel; opening
    /// replaces any open one.
    fn open_panel_menu(
        &mut self,
        panel: Arc<dyn PanelView>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().clone();
        let zoomed = self.zoomed;
        let zoomable = self.zoomable(cx).map_or(false, |v| v.menu_visible());
        // Hands focus back to the panel when the menu dismisses.
        let focus_handle = self.focus_handle(cx);

        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            panel
                .dropdown_menu(menu.action_context(focus_handle), window, cx)
                .separator()
                .item(
                    PopupMenuItem::new(if zoomed { "Zoom Out" } else { "Zoom In" })
                        .disabled(!zoomable)
                        // Carries the action only so the row shows its chord;
                        // the on_click below is what actually runs (a menu
                        // item with a handler ignores the action on click).
                        .action(Box::new(ToggleZoom))
                        .on_click({
                            let view = view.clone();
                            move |_, window, cx| {
                                view.update(cx, |this, cx| {
                                    this.on_action_toggle_zoom(&ToggleZoom, window, cx)
                                });
                            }
                        }),
                )
        });

        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            cx.notify();
        });
        self.context_menu = Some((position, menu, subscription));
        cx.notify();
    }

    /// Tears down the open context menu and hands focus back to the panel,
    /// the same way the menu's own dismiss would. Wired to the occluding
    /// backdrop so an outside press always closes the menu.
    fn dismiss_context_menu(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context_menu.take().is_some() {
            window.focus(&self.focus_handle(cx));
            cx.notify();
        }
    }

    fn render_toolbar(
        &mut self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return div();
        }

        let zoomed = self.zoomed;
        let view = cx.entity().clone();
        let zoomable_toolbar_visible = state.zoomable.map_or(false, |v| v.toolbar_visible());

        h_flex()
            .gap_1()
            .occlude()
            .when_some(self.toolbar_buttons(window, cx), |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|btn| btn.xsmall().ghost().tab_stop(false)),
                )
            })
            .map(|this| {
                let value = if zoomed {
                    Some(("zoom-out", IconName::Minimize, "Zoom Out"))
                } else if zoomable_toolbar_visible {
                    Some(("zoom-in", IconName::Maximize, "Zoom In"))
                } else {
                    None
                };

                if let Some((id, icon, tooltip)) = value {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .tooltip_with_action(tooltip, &ToggleZoom, None)
                            .when(zoomed, |this| this.selected(true))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.on_action_toggle_zoom(&ToggleZoom, window, cx)
                            })),
                    )
                } else {
                    this
                }
            })
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .dropdown_menu({
                        let zoomable = state.zoomable.map_or(false, |v| v.menu_visible());

                        // The panel's own dropdown carries close on its
                        // tail, so only zoom joins it here.
                        move |menu, window, cx| {
                            view.update(cx, |this, cx| {
                                this.dropdown_menu(menu, window, cx)
                                    .separator()
                                    .menu_with_disabled(
                                        if zoomed { "Zoom Out" } else { "Zoom In" },
                                        Box::new(ToggleZoom),
                                        !zoomable,
                                    )
                            })
                        }
                    })
                    .anchor(Corner::TopRight),
            )
    }

    fn render_dock_toggle_button(
        &self,
        placement: DockPlacement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        if self.zoomed {
            return None;
        }

        let dock_area = self.dock_area.upgrade()?.read(cx);
        if !dock_area.toggle_button_visible {
            return None;
        }
        if !dock_area.is_dock_collapsible(placement, cx) {
            return None;
        }

        let view_entity_id = cx.entity().entity_id();
        let toggle_button_panels = dock_area.toggle_button_panels;

        // Check if current TabPanel's entity_id matches the one stored in DockArea for this placement
        if !match placement {
            DockPlacement::Left => {
                dock_area.left_dock.is_some() && toggle_button_panels.left == Some(view_entity_id)
            }
            DockPlacement::Right => {
                dock_area.right_dock.is_some() && toggle_button_panels.right == Some(view_entity_id)
            }
            DockPlacement::Bottom => {
                dock_area.bottom_dock.is_some()
                    && toggle_button_panels.bottom == Some(view_entity_id)
            }
            DockPlacement::Center => unreachable!(),
        } {
            return None;
        }

        let is_open = dock_area.is_dock_open(placement, cx);

        let icon = match placement {
            DockPlacement::Left => {
                if is_open {
                    IconName::PanelLeft
                } else {
                    IconName::PanelLeftOpen
                }
            }
            DockPlacement::Right => {
                if is_open {
                    IconName::PanelRight
                } else {
                    IconName::PanelRightOpen
                }
            }
            DockPlacement::Bottom => {
                if is_open {
                    IconName::PanelBottom
                } else {
                    IconName::PanelBottomOpen
                }
            }
            DockPlacement::Center => unreachable!(),
        };

        Some(
            Button::new(SharedString::from(format!("toggle-dock:{:?}", placement)))
                .icon(icon)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .tooltip(match is_open {
                    true => "Collapse",
                    false => "Expand",
                })
                .on_click(cx.listener({
                    let dock_area = self.dock_area.clone();
                    move |_, _, window, cx| {
                        _ = dock_area.update(cx, |dock_area, cx| {
                            dock_area.toggle_dock(placement, window, cx);
                        });
                    }
                })),
        )
    }

    fn render_title_bar(
        &mut self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().clone();

        let Some(dock_area) = self.dock_area.upgrade() else {
            return div().into_any_element();
        };

        let left_dock_button = self.render_dock_toggle_button(DockPlacement::Left, window, cx);
        let bottom_dock_button = self.render_dock_toggle_button(DockPlacement::Bottom, window, cx);
        let right_dock_button = self.render_dock_toggle_button(DockPlacement::Right, window, cx);
        let has_extend_dock_button = left_dock_button.is_some() || bottom_dock_button.is_some();

        let is_bottom_dock = bottom_dock_button.is_some();

        let panel_style = dock_area.read(cx).panel_style;
        let visible_panels = self.visible_panels(cx).collect::<Vec<_>>();

        if visible_panels.len() == 1 && panel_style == PanelStyle::default() {
            let panel = visible_panels.get(0).unwrap();

            if !panel.visible(cx) {
                return div().into_any_element();
            }

            // A lone panel gets no tab chrome at all: its management lives
            // in the right-click menu, and panels host their own controls
            // when solo (see `panels_count`). Only dock toggle buttons
            // still force a bar.
            if !has_extend_dock_button && right_dock_button.is_none() {
                return div().into_any_element();
            }

            let title_suffix = panel.title_suffix(window, cx);
            let title_style = panel.title_style(cx);

            return h_flex()
                .justify_between()
                .line_height(rems(1.0))
                .h(px(30.))
                .py_2()
                .pl_3()
                .pr_2()
                .when(left_dock_button.is_some(), |this| this.pl_2())
                .when(right_dock_button.is_some(), |this| this.pr_2())
                .when_some(title_style, |this, theme| {
                    this.bg(theme.background).text_color(theme.foreground)
                })
                .when(has_extend_dock_button, |this| {
                    this.child(
                        h_flex()
                            .flex_shrink_0()
                            .mr_1()
                            .gap_1()
                            .children(left_dock_button)
                            .children(bottom_dock_button),
                    )
                })
                .child(
                    div()
                        .id("tab")
                        .flex_1()
                        .min_w_16()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(panel.title(window, cx))
                        .when(state.draggable, |this| {
                            this.on_drag(
                                DragPanel {
                                    panel: panel.clone(),
                                    tab_panel: view,
                                },
                                |drag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| drag.clone())
                                },
                            )
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener({
                                let panel = panel.clone();
                                move |this, event: &MouseDownEvent, window, cx| {
                                    this.open_panel_menu(panel.clone(), event.position, window, cx);
                                }
                            }),
                        )
                        // The solo title reads as the panel's tab, so it
                        // arms a middle-button move like one.
                        .when(state.droppable, |this| {
                            this.on_mouse_down(
                                MouseButton::Middle,
                                cx.listener({
                                    let panel = panel.clone();
                                    move |this, event: &MouseDownEvent, _, _| {
                                        this.pending_middle_drag = Some((
                                            event.position,
                                            panel.clone(),
                                            MouseButton::Middle,
                                        ));
                                    }
                                }),
                            )
                            // Alt+Left arms the same move, but only when the
                            // built-in drag above isn't riding the left
                            // button; two drags off one press would fight.
                            .when(!state.draggable, |this| {
                                this.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener({
                                        let panel = panel.clone();
                                        move |this, event: &MouseDownEvent, _, _| {
                                            if !event.modifiers.alt {
                                                return;
                                            }
                                            this.pending_middle_drag = Some((
                                                event.position,
                                                panel.clone(),
                                                MouseButton::Left,
                                            ));
                                        }
                                    }),
                                )
                            })
                        }),
                )
                .children(title_suffix)
                .children(right_dock_button)
                .into_any_element();
        }

        let tabs_count = self.panels.len();

        TabBar::new("tab-bar")
            .tab_item_top_offset(-px(1.))
            .track_scroll(&self.tab_bar_scroll_handle)
            .when(has_extend_dock_button, |this| {
                this.prefix(
                    h_flex()
                        .items_center()
                        .top_0()
                        // Right -1 for avoid border overlap with the first tab
                        .right(-px(1.))
                        .border_r_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .children(left_dock_button)
                        .children(bottom_dock_button),
                )
            })
            .children(self.panels.iter().enumerate().filter_map(|(ix, panel)| {
                let mut active = state.active_panel.as_ref() == Some(panel);
                let droppable = self.collapsed;

                if !panel.visible(cx) {
                    return None;
                }

                // Always not show active tab style, if the panel is collapsed
                if self.collapsed {
                    active = false;
                }

                Some(
                    Tab::default()
                        .when(!has_extend_dock_button && ix == 0, |this| {
                            // Right 1px for avoid border overlap with the first tab
                            this.right(px(1.))
                        })
                        .map(|this| {
                            if let Some(tab_name) = panel.tab_name(cx) {
                                this.child(tab_name)
                            } else {
                                this.child(panel.title(window, cx))
                            }
                        })
                        .selected(active)
                        // Middle button, anywhere on the tab: a click closes
                        // it, dragging past the threshold moves the panel
                        // (the root's mouse-move handler takes it from here).
                        .on_mouse_down(
                            MouseButton::Middle,
                            cx.listener({
                                let panel = panel.clone();
                                move |this, event: &MouseDownEvent, _, _| {
                                    this.pending_middle_drag =
                                        Some((event.position, panel.clone(), MouseButton::Middle));
                                }
                            }),
                        )
                        // Alt+Left arms the same move, but only when the
                        // built-in drag below isn't riding the left button
                        // (a collapsed dock's tabs, or a tab that can't
                        // drag); two drags off one press would fight.
                        .when(self.collapsed || !state.draggable, |this| {
                            this.on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let panel = panel.clone();
                                    move |this, event: &MouseDownEvent, _, _| {
                                        if !event.modifiers.alt {
                                            return;
                                        }
                                        this.pending_middle_drag = Some((
                                            event.position,
                                            panel.clone(),
                                            MouseButton::Left,
                                        ));
                                    }
                                }),
                            )
                        })
                        .on_mouse_up(
                            MouseButton::Middle,
                            cx.listener({
                                let panel = panel.clone();
                                move |this, _: &MouseUpEvent, window, cx| {
                                    // Still armed means no drag started.
                                    let Some((_, pending, button)) =
                                        this.pending_middle_drag.take()
                                    else {
                                        return;
                                    };
                                    // Only a middle press closes; a stray
                                    // middle release over an Alt+Left arm
                                    // just disarms.
                                    if button != MouseButton::Middle {
                                        return;
                                    }
                                    if pending.view() != panel.view() {
                                        return;
                                    }
                                    if this.can_close_panel(&panel, cx) {
                                        this.remove_panel(panel.clone(), window, cx);
                                    }
                                }
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener({
                                let panel = panel.clone();
                                move |this, event: &MouseDownEvent, window, cx| {
                                    this.open_panel_menu(panel.clone(), event.position, window, cx);
                                }
                            }),
                        )
                        .on_click(cx.listener({
                            let is_collapsed = self.collapsed;
                            let dock_area = self.dock_area.clone();
                            move |view, _, window, cx| {
                                view.set_active_ix(ix, window, cx);

                                // Open dock if clicked on the collapsed bottom dock
                                if is_bottom_dock && is_collapsed {
                                    _ = dock_area.update(cx, |dock_area, cx| {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    });
                                }
                            }
                        }))
                        .when(!droppable, |this| {
                            this.when(state.draggable, |this| {
                                this.on_drag(
                                    DragPanel::new(panel.clone(), view.clone()),
                                    |drag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    },
                                )
                            })
                            .when(state.droppable, |this| {
                                this.drag_over::<DragPanel>(|this, _, _, cx| {
                                    this.rounded_l_none()
                                        .border_l_2()
                                        .border_r_0()
                                        .border_color(cx.theme().drag_border)
                                })
                                .on_drop(cx.listener(
                                    move |this, drag: &DragPanel, window, cx| {
                                        this.will_split_placement = None;
                                        this.on_drop(drag, Some(ix), true, window, cx)
                                    },
                                ))
                            })
                        }),
                )
            }))
            .last_empty_space(
                // empty space to allow move to last tab right
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow()
                    .min_w_16()
                    .when(state.droppable, |this| {
                        this.drag_over::<DragPanel>(|this, _, _, cx| {
                            this.bg(cx.theme().drop_target)
                        })
                        .on_drop(cx.listener(
                            move |this, drag: &DragPanel, window, cx| {
                                this.will_split_placement = None;

                                let ix = if drag.tab_panel == view {
                                    Some(tabs_count - 1)
                                } else {
                                    None
                                };

                                this.on_drop(drag, ix, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(!self.collapsed, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .border_l_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .gap_1()
                        .children(
                            self.active_panel(cx)
                                .and_then(|panel| panel.title_suffix(window, cx)),
                        )
                        .child(self.render_toolbar(state, window, cx))
                        .when_some(right_dock_button, |this, btn| this.child(btn)),
                )
            })
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        state: &TabState,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return Empty {}.into_any_element();
        }

        let Some(active_panel) = state.active_panel.as_ref() else {
            return Empty {}.into_any_element();
        };

        let is_render_in_tabs = self.panels.len() > 1 && self.inner_padding(cx);

        // A middle drag in flight with the pointer over this panel: the
        // placement its release would use, for the same split preview the
        // built-in drag gets. The root edge bands outrank the group, so
        // no preview here while one of them claims the pointer.
        let middle_target = self.dock_area.upgrade().and_then(|dock| {
            let dock = dock.read(cx);
            let drag = dock.middle_drag.as_ref()?;
            if dock.edge_placement(drag.position).is_some() {
                return None;
            }
            let bounds = self.content_bounds.get();
            bounds
                .contains(&drag.position)
                .then(|| Self::placement_for(drag.position, bounds))
        });
        let preview_placement = middle_target.unwrap_or(self.will_split_placement);

        v_flex()
            .id("active-panel")
            .group("")
            .flex_1()
            .child({
                let content_bounds = self.content_bounds.clone();
                canvas(
                    move |bounds, _, _| content_bounds.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            })
            // The panel body answers right-click with the same menu as its
            // tab, so a lone chrome-less panel stays manageable. A panel
            // whose content serves context menus of its own opts out: the
            // content's menu opens on the same bubble-phase right-click
            // without stopping it, so answering here too would stack the
            // panel dropdown on top of it.
            .when(!active_panel.content_context_menu(cx), |this| {
                this.on_mouse_down(
                    MouseButton::Right,
                    cx.listener({
                        let panel = active_panel.clone();
                        move |this, event: &MouseDownEvent, window, cx| {
                            this.open_panel_menu(panel.clone(), event.position, window, cx);
                        }
                    }),
                )
            })
            .when(is_render_in_tabs, |this| this.pt_2())
            .child(
                div()
                    .id("tab-content")
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .flex_1()
                    .child(
                        active_panel
                            .view()
                            .cached(StyleRefinement::default().absolute().size_full()),
                    ),
            )
            // A middle-button press on the body arms a move of the active
            // panel; the root's mouse-move handler starts the drag past the
            // threshold. Gated on droppable (= not locked) rather than
            // draggable, so a section's last panel can still move out.
            .when(state.droppable, |this| {
                this.on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if let Some(panel) = this.active_panel(cx) {
                            this.pending_middle_drag =
                                Some((event.position, panel, MouseButton::Middle));
                        }
                    }),
                )
                // Alt+Left arms the same move, and it is the rearrange
                // gesture on the body, so claim the press in the capture
                // phase: panel content must not react to a grab (an
                // empty-state's click-to-open, say), and without the down
                // event no click fires on the release either.
                .capture_any_mouse_down(cx.listener(
                    |this, event: &MouseDownEvent, _, cx| {
                        if event.button != MouseButton::Left || !event.modifiers.alt {
                            return;
                        }
                        cx.stop_propagation();
                        if let Some(panel) = this.active_panel(cx) {
                            this.pending_middle_drag =
                                Some((event.position, panel, MouseButton::Left));
                        }
                    },
                ))
            })
            .when(state.droppable, |this| {
                this.on_drag_move(cx.listener(Self::on_panel_drag_move))
                    // A release of the drag's own button over this panel
                    // lands the drag here.
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_move_release))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_move_release))
                    .child(
                        div()
                            .invisible()
                            .absolute()
                            .bg(cx.theme().drop_target)
                            .map(|this| match preview_placement {
                                Some(placement) => {
                                    let size = relative(0.5);
                                    match placement {
                                        Placement::Left => this.left_0().top_0().bottom_0().w(size),
                                        Placement::Right => {
                                            this.right_0().top_0().bottom_0().w(size)
                                        }
                                        Placement::Top => this.top_0().left_0().right_0().h(size),
                                        Placement::Bottom => {
                                            this.bottom_0().left_0().right_0().h(size)
                                        }
                                    }
                                }
                                None => this.top_0().left_0().size_full(),
                            })
                            .group_drag_over::<DragPanel>("", |this| this.visible())
                            .when(middle_target.is_some(), |this| this.visible())
                            .on_drop(cx.listener(|this, drag: &DragPanel, window, cx| {
                                this.on_drop(drag, None, true, window, cx)
                            })),
                    )
            })
            .into_any_element()
    }

    /// The split direction for a pointer inside bounds. A central rectangle
    /// merges into the tabs (None). The top and bottom claim a band that hugs
    /// the real edge, capped so a tall panel doesn't hand its whole upper or
    /// lower half to a vertical split; the interior height between the bands
    /// splits left or right. The old envelope style compared normalized
    /// distances, so the top and bottom wedges widened toward the vertical
    /// center and pinched side-by-side into a sliver right before the edge
    /// took over - worst on thin panels, where that sliver was the whole
    /// panel.
    fn placement_for(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<Placement> {
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return None;
        }
        let x = position.x - bounds.left();
        let y = position.y - bounds.top();
        let fx = x / bounds.size.width;
        let fy = y / bounds.size.height;

        let quarter = bounds.size.height * 0.25;
        let band = if quarter < px(64.) { quarter } else { px(64.) };
        if y < band {
            return Some(Placement::Top);
        }
        if bounds.size.height - y < band {
            return Some(Placement::Bottom);
        }

        if (0.35..0.65).contains(&fx) && (0.35..0.65).contains(&fy) {
            return None;
        }
        Some(if fx < 0.5 {
            Placement::Left
        } else {
            Placement::Right
        })
    }

    /// Calculate the split direction based on the current mouse position
    fn on_panel_drag_move(
        &mut self,
        drag: &DragMoveEvent<DragPanel>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.will_split_placement = Self::placement_for(drag.event.position, drag.bounds);
        cx.notify()
    }

    /// A release over the panel body: if a hand-rolled drag is riding the
    /// released button, take it from the dock and land it here.
    fn on_move_release(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_middle_drag = None;
        let Some(dock) = self.dock_area.upgrade() else {
            return;
        };
        let Some(drag) = dock.update(cx, |dock, _| {
            if dock.middle_drag.as_ref().map(|drag| drag.button) != Some(event.button) {
                return None;
            }
            dock.middle_drag.take()
        }) else {
            return;
        };
        cx.stop_propagation();
        self.on_middle_drop(drag, event.position, window, cx);
    }

    /// Complete a hand-rolled drag released over this panel: the same
    /// placement math and drop path as the built-in drag.
    fn on_middle_drop(
        &mut self,
        drag: MiddleDrag,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = drag.source.upgrade() else {
            return;
        };
        self.will_split_placement = Self::placement_for(position, self.content_bounds.get());
        let drag = DragPanel::new(drag.panel, source);
        self.on_drop(&drag, None, true, window, cx);
    }

    /// Handle the drop event when dragging a panel
    ///
    /// - `active` - When true, the panel will be active after the drop
    fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();

        // If target is same tab, and it is only one panel, do nothing.
        if is_same_tab && ix.is_none() {
            if self.will_split_placement.is_none() {
                return;
            } else {
                if self.panels.len() == 1 {
                    return;
                }
            }
        }

        // Here is looks like remove_panel on a same item, but it difference.
        //
        // We must to split it to remove_panel, unless it will be crash by error:
        // Cannot update ui::dock::tab_panel::TabPanel while it is already being updated
        //
        // `ix` was computed at render time, before the detach below shifts
        // the vec; capture where the panel sat so a rightward same-group
        // drag doesn't land one slot past the indicator.
        let same_tab_from_ix = if is_same_tab {
            let panel_view = panel.view();
            self.panels.iter().position(|p| p.view() == panel_view)
        } else {
            None
        };
        if is_same_tab {
            self.detach_panel(panel.clone(), window, cx);
        } else {
            let _ = drag.tab_panel.update(cx, |view, cx| {
                view.detach_panel(panel.clone(), window, cx);
                view.remove_self_if_empty(window, cx);
            });
        }

        // Insert into new tabs
        if let Some(placement) = self.will_split_placement {
            self.split_panel(panel, placement, None, window, cx);
        } else {
            if let Some(mut ix) = ix {
                if same_tab_from_ix.is_some_and(|from_ix| from_ix < ix) {
                    ix -= 1;
                }
                self.insert_panel_at(panel, ix, window, cx)
            } else {
                self.add_panel_with_active(panel, active, window, cx)
            }
        }

        self.remove_self_if_empty(window, cx);
        cx.emit(PanelEvent::LayoutChanged);
    }

    /// Add panel with split placement
    fn split_panel(
        &self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = self.dock_area.clone();
        // wrap the panel in a TabPanel
        let new_tab_panel = cx.new(|cx| Self::new(None, dock_area.clone(), window, cx));
        new_tab_panel.update(cx, |view, cx| {
            view.add_panel(panel, window, cx);
        });

        let stack_panel = match self.stack_panel.as_ref().and_then(|panel| panel.upgrade()) {
            Some(panel) => panel,
            None => return,
        };

        let parent_axis = stack_panel.read(cx).axis;

        let ix = stack_panel
            .read(cx)
            .index_of_panel(Arc::new(cx.entity().clone()))
            .unwrap_or_default();

        if parent_axis.is_vertical() && placement.is_vertical() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else if parent_axis.is_horizontal() && placement.is_horizontal() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else {
            // 1. Create new StackPanel with new axis
            // 2. Move cx.entity() from parent StackPanel to the new StackPanel
            // 3. Add the new TabPanel to the new StackPanel at the correct index
            // 4. Add new StackPanel to the parent StackPanel at the correct index
            let tab_panel = cx.entity().clone();

            // Try to use the old stack panel, not just create a new one, to avoid too many nested stack panels
            let new_stack_panel = if stack_panel.read(cx).panels_len() <= 1 {
                stack_panel.update(cx, |view, cx| {
                    view.remove_all_panels(window, cx);
                    view.set_axis(placement.axis(), window, cx);
                });
                stack_panel.clone()
            } else {
                cx.new(|cx| {
                    let mut panel = StackPanel::new(placement.axis(), window, cx);
                    panel.parent = Some(stack_panel.downgrade());
                    panel
                })
            };

            new_stack_panel.update(cx, |view, cx| match placement {
                Placement::Left | Placement::Top => {
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                }
                Placement::Right | Placement::Bottom => {
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                }
            });

            if stack_panel != new_stack_panel {
                stack_panel.update(cx, |view, cx| {
                    view.replace_panel(
                        Arc::new(tab_panel.clone()),
                        new_stack_panel.clone(),
                        window,
                        cx,
                    );
                });
            }

            cx.spawn_in(window, async move |_, cx| {
                cx.update(|window, cx| {
                    tab_panel.update(cx, |view, cx| view.remove_self_if_empty(window, cx))
                })
            })
            .detach()
        }

        cx.emit(PanelEvent::LayoutChanged);
    }

    fn focus_active_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx).focus(window);
        }
    }

    /// The keyboard path into zoom; same behavior as the menu item.
    pub(crate) fn toggle_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_action_toggle_zoom(&ToggleZoom, window, cx);
    }

    /// Zoom out only; a no-op when not zoomed, so callers can fire it
    /// blindly without flipping an unzoomed group in.
    pub(crate) fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoomed {
            self.on_action_toggle_zoom(&ToggleZoom, window, cx);
        }
    }

    fn on_action_toggle_zoom(
        &mut self,
        _: &ToggleZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zoomable(cx).is_none() {
            return;
        }

        if !self.zoomed {
            cx.emit(PanelEvent::ZoomIn)
        } else {
            cx.emit(PanelEvent::ZoomOut)
        }
        self.zoomed = !self.zoomed;

        cx.spawn_in(window, {
            let zoomed = self.zoomed;
            async move |view, cx| {
                _ = cx.update(|window, cx| {
                    _ = view.update(cx, |view, cx| {
                        view.set_zoomed(zoomed, window, cx);
                    });
                });
            }
        })
        .detach();
    }

    fn on_action_close_panel(
        &mut self,
        _: &ClosePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.closable(cx) {
            return;
        }
        if let Some(panel) = self.active_panel(cx) {
            self.remove_panel(panel, window, cx);
        }

        // Remove self from the parent DockArea.
        // This is ensure to remove from Tiles
        if self.panels.is_empty() && self.in_tiles {
            let tab_panel = Arc::new(cx.entity());
            window.defer(cx, {
                let dock_area = self.dock_area.clone();
                move |window, cx| {
                    _ = dock_area.update(cx, |this, cx| {
                        this.remove_panel_from_all_docks(tab_panel, window, cx);
                    });
                }
            });
        }
    }

    // Bind actions to the tab panel, only when the tab panel is not collapsed.
    //
    // ToggleZoom deliberately isn't bound here: rox routes the zoom chord
    // through the dock's toggle_zoom_active so it acts on the last-clicked
    // group, not the focused one. Handling it here too would double-toggle
    // it back to a no-op. The zoom button and menu items call
    // on_action_toggle_zoom directly, so they don't need the binding.
    fn bind_actions(&self, cx: &mut Context<Self>) -> Div {
        v_flex().when(!self.collapsed, |this| {
            this.on_action(cx.listener(Self::on_action_close_panel))
        })
    }
}

impl Focusable for TabPanel {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for TabPanel {}
impl EventEmitter<PanelEvent> for TabPanel {}
impl Render for TabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let focus_handle = self.focus_handle(cx);
        let active_panel = self.active_panel(cx);
        let state = TabState {
            draggable: self.draggable(cx),
            droppable: self.droppable(cx),
            zoomable: self.zoomable(cx),
            active_panel,
        };

        self.bind_actions(cx)
            .id("tab-panel")
            .track_focus(&focus_handle)
            .tab_group()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            // Record this group as the last one clicked; the dock's
            // keyboard zoom targets it. Capture phase, so tabs and panel
            // content that stop mouse events can't hide the click.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                let weak = cx.entity().downgrade();
                _ = this
                    .dock_area
                    .update(cx, |dock, _| dock.active_tab_panel = Some(weak));
            }))
            // An armed press (middle or Alt+Left, on a tab or the body)
            // becomes a panel move once the pointer travels past the
            // threshold. Lives on the root so a fast pull off a small tab
            // still catches.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let Some((start, _, button)) = this.pending_middle_drag.as_ref() else {
                    return;
                };
                if event.pressed_button != Some(*button) {
                    this.pending_middle_drag = None;
                    return;
                }
                if (event.position - *start).magnitude() <= MIDDLE_DRAG_THRESHOLD {
                    return;
                }
                // Deliberately looser than draggable: a section's last panel
                // (the lone center panel, say) may still move to another
                // section. Dropping it on itself is a no-op, so this can't
                // strand anything the built-in drag's guard protects.
                if this.is_locked(cx) {
                    this.pending_middle_drag = None;
                    return;
                }
                let Some(dock) = this.dock_area.upgrade() else {
                    return;
                };
                let Some((_, panel, button)) = this.pending_middle_drag.take() else {
                    return;
                };
                let tabs = cx.entity();
                let chip = cx.new(|_| DragPanel::new(panel.clone(), tabs.clone()));
                let drag = MiddleDrag {
                    panel,
                    source: tabs.downgrade(),
                    chip,
                    position: event.position,
                    button,
                };
                dock.update(cx, |dock, _| dock.middle_drag = Some(drag));
                window.refresh();
            }))
            // Any unclaimed release of an arming button disarms; runs after
            // the tab and body handlers since bubble goes children first.
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.pending_middle_drag = None;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.pending_middle_drag = None;
                }),
            )
            .child(self.render_title_bar(&state, window, cx))
            .child(self.render_active_panel(&state, window, cx))
            // Same structure as gpui-component's ContextMenu element: a
            // window-sized occluding layer swallows the dismissing click,
            // the inner anchored pins the menu to the pointer. The layer also
            // dismisses on its own press: the menu's on_mouse_down_out is
            // unreliable through the deferred overlay (and the root menu skips
            // its own dismiss while a submenu is open), so an outside click
            // would otherwise leave the menu stuck open, holding focus. A
            // press on the menu or an open submenu hits their occluding
            // hitbox first, so only genuine outside presses reach this.
            .when_some(self.context_menu.as_ref(), |this, (position, menu, _)| {
                this.child(
                    deferred(
                        anchored().child(
                            div()
                                .w(window.bounds().size.width)
                                .h(window.bounds().size.height)
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(Self::dismiss_context_menu),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(Self::dismiss_context_menu),
                                )
                                .on_mouse_down(
                                    MouseButton::Middle,
                                    cx.listener(Self::dismiss_context_menu),
                                )
                                .child(
                                    anchored()
                                        .position(*position)
                                        .snap_to_window_with_margin(px(8.))
                                        .child(menu.clone()),
                                ),
                        ),
                    )
                    .with_priority(1),
                )
            })
    }
}
