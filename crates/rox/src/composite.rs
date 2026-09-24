//! Shared plumbing for the composition panels (group, overlay, slide,
//! drawer), which host other panels inside one dock slot and render them
//! themselves. Children serialize into [`PanelState::children`] and rebuild
//! through the panel registry. The dock never sees a hosted child, so there's
//! no tab drag, zoom, or pop-out per slot; the catalog menus fill slots.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    Along, App, Axis, Bounds, Context, Div, EntityId, Global, MouseButton, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, SharedString, WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{DockArea, Panel, PanelRegistry, PanelState, PanelView, TabPanel};

use crate::panel_catalog::{self as catalog, PanelDef};
use crate::panel_settings;
use crate::workspace::Workspace;
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{AppState, PanelSettings};

pub type Slot = Option<Arc<dyn PanelView>>;

/// Open the focused child's settings, or the host's own. The dock's chord
/// only reaches the container, so every composite overrides
/// `Panel::open_settings` with this.
pub fn open_slot_settings<'a, P: PanelSettings>(
    slots: impl IntoIterator<Item = &'a Slot>,
    window: &mut Window,
    cx: &mut Context<P>,
) {
    let focused = slots
        .into_iter()
        .flatten()
        .find(|child| child.focus_handle(cx).contains_focused(window, cx))
        .cloned();
    match focused {
        Some(child) => child.open_settings(window, cx),
        None => rox_panel_api::panel_settings::open(cx.entity(), cx),
    }
}

/// `on_added_to` only reaches panels the dock holds directly, so the host
/// introduces its tab panel to its children from render, or their fallback
/// right-click menu goes nowhere. `introduced` keeps it to once per change.
pub fn introduce_slots<'a>(
    children: impl IntoIterator<Item = &'a Arc<dyn PanelView>>,
    tab_panel: &Option<WeakEntity<TabPanel>>,
    introduced: &mut bool,
    window: &mut Window,
    cx: &mut App,
) {
    if *introduced {
        return;
    }
    let Some(tabs) = tab_panel else {
        return;
    };
    *introduced = true;
    for child in children {
        child.on_added_to(tabs.clone(), window, cx);
    }
}

/// Right-click routes to the tab panel's fallback menu unless the child serves its own.
pub fn menu_routed_slot(
    child: &Arc<dyn PanelView>,
    tab_panel: &Option<WeakEntity<TabPanel>>,
    cx: &App,
) -> Div {
    let body = div().size_full().child(child.view());
    if child.content_context_menu(cx) {
        return body;
    }
    let Some(tabs) = tab_panel.clone() else {
        return body;
    };
    let target = child.clone();
    body.on_mouse_down(
        MouseButton::Right,
        move |event: &gpui::MouseDownEvent, window, cx| {
            let Some(tabs) = tabs.upgrade() else {
                return;
            };
            tabs.update(cx, |tabs, cx| {
                tabs.open_panel_menu(target.clone(), event.position, window, cx)
            });
        },
    )
}

/// Hosted panel to host, reported from each host's render, so a child's
/// right-click can reach the host's settings even with its controls hidden.
#[derive(Default)]
struct Hosts(HashMap<EntityId, Host>);

impl Global for Hosts {}

struct Host {
    id: EntityId,
    label: SharedString,
    /// Weak, so an entry left behind by a removed child no-ops.
    open: Rc<dyn Fn(&mut App)>,
}

/// Settles to a couple of map lookups once the slots stop changing.
pub fn report_hosted<'a, P: PanelSettings>(
    children: impl IntoIterator<Item = &'a Arc<dyn PanelView>>,
    label: &str,
    cx: &mut Context<P>,
) {
    let ids: Vec<EntityId> = children
        .into_iter()
        .map(|child| child.panel_id(cx))
        .collect();
    if ids.is_empty() {
        return;
    }
    let me = cx.entity().entity_id();
    let stale = |hosts: &Hosts| {
        ids.iter()
            .any(|id| hosts.0.get(id).map(|h| h.id) != Some(me))
    };
    if !cx.try_global::<Hosts>().is_none_or(stale) {
        return;
    }

    let weak = cx.entity().downgrade();
    let open: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
        if let Some(host) = weak.upgrade() {
            rox_panel_api::panel_settings::open(host, cx);
        }
    });
    let label = SharedString::from(label.to_string());
    let hosts = cx.default_global::<Hosts>();
    for id in ids {
        hosts.0.insert(
            id,
            Host {
                id: me,
                label: label.clone(),
                open: open.clone(),
            },
        );
    }
}

pub fn host_settings_item(menu: PopupMenu, child: EntityId, cx: &App) -> PopupMenu {
    let Some(host) = cx
        .try_global::<Hosts>()
        .and_then(|hosts| hosts.0.get(&child))
    else {
        return menu;
    };
    let open = host.open.clone();
    menu.item(
        PopupMenuItem::new(rox_i18n::t!(
            "composite-host-settings",
            host = host.label.to_string()
        ))
        .icon(Icon::default().path(icons::LAYOUT_DASHBOARD))
        .on_click(move |_, _, cx| open(cx)),
    )
}

/// An empty slot dumps as the default state, so slot positions survive.
pub fn dump_slots(slots: &[Slot], cx: &App) -> Vec<PanelState> {
    slots
        .iter()
        .map(|slot| match slot {
            Some(child) => child.dump(cx),
            None => PanelState::default(),
        })
        .collect()
}

/// An unregistered name builds the invalid-panel placeholder, keeping the dump intact.
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

/// For the layout tree. Empty slots come back as None so the tree can name the hole.
pub fn hosted_children(panel: &Arc<dyn PanelView>, cx: &App) -> Option<Vec<Slot>> {
    let view = panel.view();
    if let Ok(group) = view.clone().downcast::<crate::panels::group::GroupPanel>() {
        return Some(group.read(cx).slots().to_vec());
    }
    if let Ok(overlay) = view
        .clone()
        .downcast::<crate::panels::overlay::OverlayPanel>()
    {
        return Some(overlay.read(cx).slots().to_vec());
    }
    if let Ok(drawer) = view
        .clone()
        .downcast::<crate::panels::drawer::DrawerPanel>()
    {
        return Some(drawer.read(cx).slots().to_vec());
    }
    if let Ok(slide) = view.downcast::<crate::panels::slide::SlidePanel>() {
        return Some(slide.read(cx).slides().iter().cloned().map(Some).collect());
    }
    None
}

pub fn pick_items(
    mut menu: PopupMenu,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    // Presets lead here too, grayed by the same no-nesting rule.
    if let Some(dock) = workspace.upgrade().map(|ws| ws.read(cx).dock().downgrade()) {
        menu = crate::panel_presets::pick_submenu(menu, dock, true, window, cx, on_pick.clone());
    }
    for section in catalog::sections() {
        // Composites stay listed but grayed: one level of nesting.
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
                    rox_i18n::t!(label),
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

fn pick_item(
    menu: PopupMenu,
    def: &'static PanelDef,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    disabled: bool,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let item = PopupMenuItem::new(rox_i18n::t!(def.label)).icon(Icon::default().path(def.icon));
    if disabled {
        return menu.item(item.disabled(true));
    }
    menu.item(item.on_click(move |_, window, cx| {
        let panel = (def.build)(&state, workspace.clone(), window, cx);
        on_pick(panel, window, cx);
    }))
}

/// Out of design mode the add button hides; the Workspace page can still fill it.
pub fn empty_slot(
    id: impl Into<gpui::ElementId>,
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
        .when(settings::design_mode(), |this| {
            this.child(
                Button::new(id)
                    .icon(Icon::default().path(icons::PLUS))
                    .label(rox_i18n::t!("composite-add-panel"))
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
        })
}

/// Apply the child's min and max to its cell, the numbers the dock's splits
/// honor, since nothing else reads them for a hosted panel.
pub fn clamp_to_panel(cell: Div, child: &Slot, cx: &App) -> Div {
    let Some(child) = child else { return cell };
    let (min, max) = (child.min_size(cx), child.max_size(cx));
    cell.min_w(min.width)
        .min_h(min.height)
        .map(|d| {
            if max.width < Pixels::MAX {
                d.max_w(max.width)
            } else {
                d
            }
        })
        .map(|d| {
            if max.height < Pixels::MAX {
                d.max_h(max.height)
            } else {
                d
            }
        })
}

/// Top-right, faint until hovered. The parent grip takes the top-left.
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

pub fn parent_controls() -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(tokens::SPACE_XS)
        .opacity(0.4)
        .hover(|style| style.opacity(1.))
}

/// The host's own [`Panel::dropdown_menu`]. Needed because children take the
/// body right-click, and a solo composite gets no tab bar to hang it off.
pub fn parent_button<P: Panel>(
    tooltip: impl Into<SharedString>,
    cx: &mut Context<P>,
) -> impl IntoElement {
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

/// Replace, the child's settings, and Remove, after any rows `extend` adds.
/// A locked child keeps only its settings row.
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
            let locked = child.locked(cx);
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
            let menu = if locked {
                menu
            } else {
                menu.item(
                    PopupMenuItem::submenu(rox_i18n::t!("composite-replace"), submenu)
                        .icon(Icon::default().path(icons::REFRESH_CW)),
                )
            };
            let menu = menu.item(
                PopupMenuItem::new(rox_i18n::t!("panel-settings"))
                    .icon(Icon::default().path(icons::SETTINGS))
                    .on_click(move |_, _, cx| {
                        panel_settings::open_for_view(&settings_child, cx);
                    }),
            );
            if locked {
                return menu;
            }
            menu.item(
                PopupMenuItem::new(rox_i18n::t!("composite-remove"))
                    .icon(Icon::default().path(icons::CLOSE))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = remove_weak.upgrade() {
                            this.update(cx, |this, cx| remove(this, cx));
                        }
                    }),
            )
        })
}

/// The [`rox_panel_kit::ScrubState`] idiom, made axis-generic.
#[derive(Clone, Default)]
pub struct DividerState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
}

impl DividerState {
    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        *self.bounds.lock().unwrap() = Some(bounds);
    }

    pub fn begin(&self) {
        self.dragging.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) {
        self.dragging.store(false, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.load(Ordering::Relaxed)
    }

    /// 0 to 1; overshoot clamps so the drag never lets go.
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

/// Call from the host's paint: window handlers last one frame, the
/// [`rox_panel_kit::scrub_on_paint`] idiom. `apply` must notify.
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
            // A release outside the window never arrives; a buttonless move ends the drag.
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
