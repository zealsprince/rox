//! Shared plumbing for the layout-composition panels (group, overlay,
//! slide): panels that host other panels inside one dock slot. The dock
//! tree only knows splits and tabs, so these hosts render their children
//! themselves. A child is just an [`Arc<dyn PanelView>`] whose view goes
//! into the host's own element tree. Children serialize into the host's
//! [`PanelState::children`] and rebuild through the dock's panel registry,
//! so nesting round-trips layout dumps like any other panel, composites
//! inside composites included.
//!
//! What a hosted child gives up: the dock never sees it, so there's no
//! tab-drag into or out of a slot and no per-child zoom or pop-out. Slots
//! are filled and changed through menus built from the panel catalog
//! instead.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, svg, Along, App, Axis, Bounds, Context, Div, EntityId, Global,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, WeakEntity, Window,
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

/// One hosted slot: a live child panel, or empty and showing the add
/// affordance.
pub type Slot = Option<Arc<dyn PanelView>>;

/// Hand a host's tab panel down to its hosted children, once per change.
///
/// `on_added_to` is only ever called on panels the dock holds directly, so
/// a child in a composite slot is never told which tab panel it's in, and
/// anything it routes there (its fallback right-click menu above all)
/// quietly goes nowhere. The host knows, so it passes the introduction
/// along from render, the one place with a window in hand on every path a
/// slot can change through. `introduced` is the host's own once-flag:
/// cleared when its tab panel or a slot turns over, set here, so a settled
/// layout pays one bool check per frame.
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

/// A hosted child's view, with its right-click routed to the hosting tab
/// panel's fallback menu when the child doesn't serve a content menu of
/// its own. That's the exact overlay a lone docked panel gets. Children
/// that do serve one keep the click untouched.
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

/// Which composite a hosted panel is in. A host reports its slots as it
/// renders, so a child's own right-click can get to the panel that holds it.
/// The dock never sees a hosted child, so without this a host with its
/// corner controls hidden has no route to its settings at all, and even with
/// them showing this is the shorter one.
#[derive(Default)]
struct Hosts(HashMap<EntityId, Host>);

impl Global for Hosts {}

struct Host {
    /// The host itself, so a re-report can tell a fresh entry from its own.
    id: EntityId,
    /// What the row calls it: the host's rename when it has one, its panel
    /// name otherwise.
    label: SharedString,
    /// Opens the host's settings window. Holds the host weakly, so an entry
    /// left behind by a removed child just no-ops.
    open: Rc<dyn Fn(&mut App)>,
}

/// Record a composite as the host of its filled slots. Called from the
/// host's render, which is the one place that always sees the current
/// children; the work settles to a couple of map lookups once the slots stop
/// changing.
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

/// The row that opens a hosted panel's host settings, for the end of the
/// child's own menu. Nothing at all when the panel isn't hosted, which is
/// every panel the dock holds directly.
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

/// Serialize a host's slots in order. An empty slot dumps as the default
/// (empty-named) state, so slot positions are preserved through the
/// round-trip.
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
/// isn't a composite. The settings window's layout tree shows hosted
/// children under their host's row through this; an empty slot comes back
/// as None so the tree can name the hole. The slide deck has no holes, so
/// its entries are all Some.
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
    // The saved panels lead the list here too, grayed by the same nesting
    // rule the arrangement section is: a preset of a composite is still a
    // composite.
    if let Some(dock) = workspace.upgrade().map(|ws| ws.read(cx).dock().downgrade()) {
        menu = crate::panel_presets::pick_submenu(menu, dock, true, window, cx, on_pick.clone());
    }
    for section in catalog::sections() {
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
/// disabled row shows grayed with no click, for the panels that can't go
/// in this slot (a composite inside a composite).
fn pick_item(
    menu: PopupMenu,
    def: &'static PanelDef,
    state: AppState,
    workspace: WeakEntity<Workspace>,
    disabled: bool,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let item =
        PopupMenuItem::new(rox_i18n::t!(def.label)).icon(Icon::default().path(def.icon));
    if disabled {
        return menu.item(item.disabled(true));
    }
    menu.item(item.on_click(move |_, window, cx| {
        let panel = (def.build)(&state, workspace.clone(), window, cx);
        on_pick(panel, window, cx);
    }))
}

/// An empty slot's body: a dashed stand-in with an Add Panel dropdown
/// over the catalog. Fills whatever cell the host gives it. Out of design
/// mode the button goes and the dashed mark stands alone: filling the slot
/// is a layout edit, and the Workspace page's tree still does it.
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

/// Hold a slot's cell to the size the child asks for. A host lays its
/// children out itself, so nothing else reads a hosted panel's min and max:
/// without this, the size settings on a child inside a group say one thing
/// and the host draws another. These are the same numbers the dock's splits
/// honor for a docked panel, so a panel keeps its size wherever it ends up.
/// An unset cap comes back as [`Pixels::MAX`] and is left off the cell
/// rather than written out as a bound.
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

/// The wrapper for a host's per-slot floating controls: pinned to the
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

/// The wrapper for the composite's own grip: the top-left corner, clear of
/// the per-slot controls on the right, faint until hovered.
pub fn parent_controls() -> Div {
    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(tokens::SPACE_XS)
        .opacity(0.4)
        .hover(|style| style.opacity(1.))
}

/// The composite's own menu button: opens the host's [`Panel::dropdown_menu`],
/// the very menu the dock's tab chrome shows for it. The parent grip keeps
/// split, swap, rename, settings, and close reachable from inside the
/// panel, which matters when the composite is solo and the dock draws no
/// tab bar to hang that menu off. Content panels set
/// `content_context_menu` so a right-click over a child opens the child's
/// own menu, not the parent's; this button is how the parent stays
/// managed once that body route is handed to the children. Drawn with the
/// layout mark so the grip reads as the container, not a child.
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

/// A filled slot's menu button: Replace (the catalog as a flyout), the
/// child's Panel Settings, and Remove, with `extend` prepending any
/// host-specific rows (a slide's reorder moves). Replace and Remove go
/// back to the host through the callbacks; the settings route goes
/// through the type-erased opener, so a child type without a settings
/// window just no-ops.
///
/// A locked child keeps its settings row and loses the two that would move
/// it: locked means pinned in place, and a slot is the hosted panel's
/// version of the tab a docked panel gets pinned into.
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

/// A group divider's live drag: where the slots container painted and
/// whether a drag is on, behind Arcs so the panel, its paint closure, and
/// the window-level handlers can all hold it. The [`rox_panel_kit::ScrubState`]
/// idiom, made axis-generic for the vertical split.
#[derive(Clone, Default)]
pub struct DividerState {
    bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    dragging: Arc<AtomicBool>,
}

impl DividerState {
    /// Remember where the slots container was painted, from its prepaint.
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

    /// Where the pointer is along the container's `axis`, 0 to 1;
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
/// the host's paint pass: window handlers only last one frame, the
/// [`rox_panel_kit::scrub_on_paint`] idiom. Applying must notify the
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
