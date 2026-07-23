//! Shared plumbing for the layout-composition panels (group, depth,
//! slide): panels that host other panels inside one dock slot. The dock
//! tree only knows splits and tabs, so these hosts render their children
//! themselves - a child is just an [`Arc<dyn PanelView>`] whose view lands
//! in the host's own element tree. Children serialize into the host's
//! [`PanelState::children`] and rebuild through the dock's panel registry,
//! so nesting round-trips layout dumps like any other panel, composites
//! inside composites included.
//!
//! What a hosted child gives up: the dock never sees it, so there is no
//! tab-drag into or out of a slot and no per-child zoom or pop-out. Slots
//! are filled and changed through menus built from the panel catalog
//! instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, svg, Along, App, Axis, Bounds, Context, Div, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{DockArea, Panel, PanelRegistry, PanelState, PanelView};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::panel_catalog::{self as catalog, PanelDef};
use crate::panel_settings;
use crate::workspace::Workspace;

/// One hosted slot: a live child panel, or empty and showing the add
/// affordance.
pub type Slot = Option<Arc<dyn PanelView>>;

/// Serialize a host's slots in order. An empty slot dumps as the default
/// (empty-named) state, so slot positions survive the round-trip.
pub fn dump_slots(slots: &[Slot], cx: &App) -> Vec<PanelState> {
    slots
        .iter()
        .map(|slot| match slot {
            Some(child) => child.dump(cx),
            None => PanelState::default(),
        })
        .collect()
}

/// Rebuild a host's slots from its dumped children through the panel
/// registry, the same route the dock takes for its own tabs. The empty
/// sentinel comes back as an empty slot; an unregistered name builds the
/// dock's invalid-panel placeholder, which keeps the dump intact.
pub fn restore_slots(
    dock_area: &WeakEntity<DockArea>,
    state: &PanelState,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Slot> {
    state
        .children
        .iter()
        .map(|child| {
            if child.panel_name.is_empty() {
                return None;
            }
            let view = PanelRegistry::build_panel(
                &child.panel_name,
                dock_area.clone(),
                child,
                &child.info,
                window,
                cx,
            );
            Some(Arc::from(view))
        })
        .collect()
}

/// The children a composite hosts, in slot order, or None when the panel
/// is no composite. The settings window's layout tree shows hosted
/// children under their host's row through this; an empty slot comes back
/// as None so the tree can name the hole. The slide deck has no holes, so
/// its entries are all Some.
pub fn hosted_children(panel: &Arc<dyn PanelView>, cx: &App) -> Option<Vec<Slot>> {
    let view = panel.view();
    if let Ok(group) = view.clone().downcast::<crate::panels::group::GroupPanel>() {
        return Some(group.read(cx).slots().to_vec());
    }
    if let Ok(depth) = view.clone().downcast::<crate::panels::depth::DepthPanel>() {
        return Some(depth.read(cx).slots().to_vec());
    }
    if let Ok(slide) = view.downcast::<crate::panels::slide::SlidePanel>() {
        return Some(slide.read(cx).slides().iter().cloned().map(Some).collect());
    }
    None
}

/// Append the catalog to a menu as pick rows: the bare center panels
/// flat, the labeled groups as flyouts, the same shape as the dock's Add
/// Panel submenu. A pick builds the panel against the workspace's state
/// and hands it to `on_pick`; where it goes is the caller's business.
pub fn pick_items(
    mut menu: PopupMenu,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    for section in catalog::CATALOG {
        // The arrangement panels stay in the slot picker but grayed: a
        // composite can't host another composite, one level of nesting.
        let disabled = catalog::is_arrangement(section);
        match section.group {
            None => {
                for def in section.panels {
                    menu = pick_item(
                        menu,
                        def,
                        state.clone(),
                        workspace.clone(),
                        disabled,
                        on_pick.clone(),
                    );
                }
            }
            Some((label, icon)) => {
                let state = state.clone();
                let workspace = workspace.clone();
                let on_pick = on_pick.clone();
                menu = menu.submenu_with_icon(
                    Some(Icon::default().path(icon)),
                    label,
                    window,
                    cx,
                    move |mut menu, _, _| {
                        for def in section.panels {
                            menu = pick_item(
                                menu,
                                def,
                                state.clone(),
                                workspace.clone(),
                                disabled,
                                on_pick.clone(),
                            );
                        }
                        menu
                    },
                );
            }
        }
    }
    menu
}

/// One catalog pick row: build the def's panel and hand it over. A
/// disabled row shows grayed with no click, for the panels that can't
/// land in this slot (a composite inside a composite).
fn pick_item(
    menu: PopupMenu,
    def: &'static PanelDef,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    disabled: bool,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let item = PopupMenuItem::new(def.label).icon(Icon::default().path(def.icon));
    if disabled {
        return menu.item(item.disabled(true));
    }
    menu.item(item.on_click(move |_, window, cx| {
        let panel = (def.build)(&state, workspace.clone(), window, cx);
        on_pick(panel, window, cx);
    }))
}

/// An empty slot's body: a dashed stand-in with an Add Panel dropdown
/// over the catalog. Fills whatever cell the host gives it.
pub fn empty_slot(
    id: &'static str,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(tokens::SPACE_SM)
        .child(
            svg()
                .path(icons::SQUARE_DASHED)
                .size(px(28.))
                .text_color(palette::text_faint()),
        )
        .child(
            Button::new(id)
                .icon(Icon::default().path(icons::PLUS))
                .label("Add Panel")
                .small()
                .outline()
                .dropdown_menu(move |menu, window, cx| {
                    pick_items(
                        menu,
                        state.clone(),
                        workspace.clone(),
                        window,
                        cx,
                        on_pick.clone(),
                    )
                }),
        )
}

/// The wrapper a host's per-slot floating controls sit in: pinned to the
/// slot's top-right corner, faint until hovered so they never fight the
/// child's own chrome for attention. Children's controls on the right,
/// the parent grip on the left, so the two never collide.
pub fn corner_controls() -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .right(tokens::SPACE_XS)
        .flex()
        .flex_row()
        .gap(tokens::SPACE_XS)
        .opacity(0.4)
        .hover(|style| style.opacity(1.))
}

/// The wrapper the composite's own grip sits in: the top-left corner,
/// clear of the per-slot controls on the right, faint until hovered.
pub fn parent_controls() -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(tokens::SPACE_XS)
        .opacity(0.4)
        .hover(|style| style.opacity(1.))
}

/// The composite's own menu button: opens the host's [`Panel::dropdown_menu`],
/// the very menu the dock's tab chrome shows for it. The parent grip - it
/// keeps split, swap, rename, settings, and close reachable from inside
/// the panel, which matters when the composite is solo and the dock draws
/// no tab bar to hang that menu off. Content panels set
/// `content_context_menu` so a right-click over a child opens the child's
/// own menu, not the parent's; this button is how the parent stays
/// managed once that body route is handed to the children. Sits under the
/// layout mark so the grip reads as the container, not a child.
pub fn parent_button<P: Panel>(tooltip: &'static str, cx: &mut Context<P>) -> impl IntoElement {
    let weak = cx.entity().downgrade();
    Button::new("composite-parent")
        .icon(Icon::default().path(icons::LAYOUT_DASHBOARD))
        .small()
        .ghost()
        .tooltip(tooltip)
        .dropdown_menu(move |menu, window, cx| match weak.upgrade() {
            Some(this) => this.update(cx, |this, cx| this.dropdown_menu(menu, window, cx)),
            None => menu,
        })
}

/// A filled slot's menu button: Replace (the catalog as a flyout), the
/// child's Panel Settings, and Remove, with `extend` prepending any
/// host-specific rows (a slide's reorder moves). Replace and Remove land
/// back on the host through the callbacks; the settings route goes
/// through the type-erased opener, so a child type without a settings
/// window just no-ops.
#[allow(clippy::too_many_arguments)]
pub fn slot_button<P: 'static>(
    id: (&'static str, usize),
    child: Arc<dyn PanelView>,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    replace: impl Fn(&mut P, Arc<dyn PanelView>, &mut Context<P>) + Clone + 'static,
    remove: impl Fn(&mut P, &mut Context<P>) + Clone + 'static,
    extend: impl Fn(PopupMenu, WeakEntity<P>) -> PopupMenu + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
    let weak = cx.entity().downgrade();
    Button::new(id)
        .icon(Icon::default().path(icons::MENU))
        .small()
        .ghost()
        .dropdown_menu(move |menu, window, cx| {
            let menu = extend(menu, weak.clone());
            let pick_weak = weak.clone();
            let replace = replace.clone();
            let submenu = PopupMenu::build(window, cx, {
                let state = state.clone();
                let workspace = workspace.clone();
                move |menu, window, cx| {
                    pick_items(menu, state, workspace, window, cx, move |panel, _, cx| {
                        if let Some(this) = pick_weak.upgrade() {
                            this.update(cx, |this, cx| replace(this, panel, cx));
                        }
                    })
                }
            });
            let settings_child = child.clone();
            let remove_weak = weak.clone();
            let remove = remove.clone();
            menu.item(
                PopupMenuItem::submenu("Replace", submenu)
                    .icon(Icon::default().path(icons::REFRESH_CW)),
            )
            .item(
                PopupMenuItem::new("Panel Settings")
                    .icon(Icon::default().path(icons::SETTINGS))
                    .on_click(move |_, _, cx| {
                        panel_settings::open_for_view(&settings_child, cx);
                    }),
            )
            .item(
                PopupMenuItem::new("Remove")
                    .icon(Icon::default().path(icons::CLOSE))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = remove_weak.upgrade() {
                            this.update(cx, |this, cx| remove(this, cx));
                        }
                    }),
            )
        })
}

/// A group divider's live drag: where the slots container painted and
/// whether a drag is on, behind Arcs so the panel, its paint closure, and
/// the window-level handlers can all hold it. The [`crate::panel::ScrubState`]
/// idiom, made axis-generic for the vertical split.
#[derive(Clone, Default)]
pub struct DividerState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
}

impl DividerState {
    /// Remember where the slots container landed, from its prepaint.
    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        *self.bounds.lock().unwrap() = Some(bounds);
    }

    /// A drag started (mouse down on the divider).
    pub fn begin(&self) {
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    /// Where the pointer lands along the container's `axis`, 0 to 1;
    /// overshoot clamps so the drag never lets go of the divider.
    fn fraction(&self, position: Point<Pixels>, axis: Axis) -> Option<f32> {
        let bounds = (*self.bounds.lock().unwrap())?;
        let extent = f32::from(bounds.size.along(axis));
        if extent <= 0.0 {
            return None;
        }
        let offset = f32::from(position.along(axis) - bounds.origin.along(axis));
        Some((offset / extent).clamp(0.0, 1.0))
    }
}

/// Keep a live divider drag following the pointer along `axis`: apply the
/// container fraction on every move, end the drag on release. Call from
/// the host's paint pass - window handlers only live one frame, the
/// [`crate::panel::scrub_on_paint`] idiom. Applying must notify the
/// entity so the next frame re-arms the handlers.
pub fn divider_on_paint(
    divider: &DividerState,
    axis: Axis,
    window: &mut Window,
    apply: impl Fn(f32, &mut App) + 'static,
) {
    if !divider.is_dragging() {
        return;
    }
    window.on_mouse_event({
        let divider = divider.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() || !divider.is_dragging() {
                return;
            }
            // A release outside the window never reaches the up handler;
            // a move without the button still held ends the drag instead.
            if event.pressed_button != Some(MouseButton::Left) {
                divider.end();
                return;
            }
            if let Some(fraction) = divider.fraction(event.position, axis) {
                apply(fraction, cx);
            }
        }
    });
    window.on_mouse_event({
        let divider = divider.clone();
        move |_: &MouseUpEvent, phase, _, _| {
            if phase.bubble() {
                divider.end();
            }
        }
    });
}
