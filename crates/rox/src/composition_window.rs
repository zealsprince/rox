//! The composition of a workspace window: its panels as they sit in splits and
//! tab groups, and the way to turn that layout into a canvas. One view,
//! [`CompositionView`], shown in a window of its own off the Panels menu and
//! as the Workspace settings page's Composition section, so the two can't
//! drift.
//!
//! The convert itself is the workspace's, behind its confirm: this view only
//! says what it does and asks for it.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyWindowHandle, App, Axis, Bounds, Context, Div, ElementId, Entity, Global,
    Pixels, ScrollHandle, SharedString, Styled, Subscription, WeakEntity, Window, WindowHandle,
    div, prelude::*, px, size,
};
use gpui_component::Root;
use gpui_component::scroll::Scrollbar;

use crate::panel_settings;
use crate::workspace::Workspace;
use rox_core::settings::{self, LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_dock::{DockArea, DockEvent, PanelView, StackPanel, TabPanel};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{icon_button, small_button};

const MIN: gpui::Size<Pixels> = gpui::Size {
    width: px(420.),
    height: px(360.),
};

const TREE_ROW_GROUP: &str = "tree-row";

/// The one Composition window, and the workspace it shows.
struct OpenComposition {
    handle: WindowHandle<Root>,
    workspace: WeakEntity<Workspace>,
}

impl Global for OpenComposition {}

/// Opened from the workspace's own update, so the dock comes in as a handle
/// rather than read off the workspace. A window already showing another
/// workspace closes for this one.
pub fn open(
    state: AppState,
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    dock: Entity<DockArea>,
    cx: &mut App,
) {
    cx.defer(move |cx| open_now(state, workspace, workspace_window, dock, cx));
}

fn open_now(
    state: AppState,
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    dock: Entity<DockArea>,
    cx: &mut App,
) {
    if let Some(open) = cx.try_global::<OpenComposition>() {
        let (handle, same) = (open.handle, open.workspace == workspace);
        let reached = handle.update(cx, |_, window, _| {
            if same {
                window.activate_window();
            } else {
                window.remove_window();
            }
        });
        if same && reached.is_ok() {
            return;
        }
    }

    let (width, height) = Settings::load()
        .windows
        .composition
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((520., 560.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let player = state.player.entity_id();
    let target = workspace.clone();
    let handle = panel::open_child_window(
        cx,
        || rox_i18n::t!("composition-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| {
            cx.new(|cx| CompositionWindow::new(player, target, workspace_window, dock, window, cx))
        },
    );
    cx.set_global(OpenComposition { handle, workspace });
}

struct CompositionWindow {
    /// For the tint: the workspace's player.
    player: gpui::EntityId,
    view: Entity<CompositionView>,
    scroll: ScrollHandle,
}

impl CompositionWindow {
    fn new(
        player: gpui::EntityId,
        workspace: WeakEntity<Workspace>,
        workspace_window: AnyWindowHandle,
        dock: Entity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The OS close button never runs remove_window.
        window.on_window_should_close(cx, |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                s.windows.composition = Some(LayoutSize {
                    width: frame.size.width.into(),
                    height: frame.size.height.into(),
                });
            });
            true
        });

        let view = cx.new(|cx| CompositionView::new(workspace, workspace_window, &dock, cx));
        CompositionWindow {
            player,
            view,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Render for CompositionWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        palette::note_focus(self.player, window.is_window_active(), cx);

        // Built inside the closure, or it reads the palette before the tint.
        panel::window_body(self.player, || {
            div()
                .size_full()
                .relative()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(
                    div()
                        .id("composition")
                        .size_full()
                        .p(tokens::SPACE_MD)
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(self.view.clone()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(Scrollbar::vertical(&self.scroll)),
                )
                .into_any_element()
        })
    }
}

/// The tree and the convert row, for one workspace window.
pub(crate) struct CompositionView {
    /// Weak, so this never keeps a closed workspace alive.
    workspace: WeakEntity<Workspace>,
    /// For the `Window` a lift rebuilds the dock in.
    workspace_window: AnyWindowHandle,
    _dock_changes: Vec<Subscription>,
}

impl CompositionView {
    pub(crate) fn new(
        workspace: WeakEntity<Workspace>,
        workspace_window: AnyWindowHandle,
        dock: &Entity<DockArea>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _dock_changes = vec![
            cx.subscribe(dock, |_, _, event: &DockEvent, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    cx.notify();
                }
            }),
            cx.observe(dock, |_, _, cx| cx.notify()),
        ];

        CompositionView {
            workspace,
            workspace_window,
            _dock_changes,
        }
    }

    fn tree(&self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) -> Div {
        let dock = workspace.read(cx).dock().clone();
        let root = dock.read(cx).items().view();

        let mut rows = Vec::new();
        self.tree_rows(root, 0, TreeSlot::Root, &dock.downgrade(), &mut rows, cx);
        div().flex().flex_col().children(rows)
    }

    /// What converting does, and the button, unless the window is already a
    /// canvas or has nothing in it yet, which the row says instead.
    fn convert_row(&self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) -> Div {
        let (canvas, empty) = {
            let ws = workspace.read(cx);
            (ws.dock_is_one_canvas(cx), ws.dock_is_empty(cx))
        };
        let note = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
        };

        let row = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .pb(tokens::SPACE_MD)
            .mb(tokens::SPACE_SM)
            .border_b_1()
            .border_color(palette::border())
            .child(div().child(rox_i18n::t!("composition-convert-title")));

        if canvas {
            return row.child(note(rox_i18n::t!("composition-convert-done")));
        }

        let row = row.child(note(rox_i18n::t!("composition-convert-body")));
        if empty {
            return row.child(note(rox_i18n::t!("composition-convert-empty")));
        }

        let workspace = workspace.downgrade();
        let workspace_window = self.workspace_window;
        row.child(note(rox_i18n::t!("composition-convert-undo")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .pt(tokens::SPACE_XS)
                    .child(small_button(
                        rox_i18n::t!("composition-convert-button"),
                        icons::LAYERS,
                        false,
                        move |_, _, cx| {
                            let workspace = workspace.clone();
                            workspace_window
                                .update(cx, |_, window, cx| {
                                    workspace
                                        .update(cx, |ws, cx| {
                                            ws.convert_layout_to_canvas(window, cx)
                                        })
                                        .ok();
                                })
                                .ok();
                        },
                    )),
            )
    }

    /// Walks the live stack and tab entities, not the dock's `DockItem` tree,
    /// which goes stale once tabs move. `dock` is the area the tree belongs
    /// to: the window's, or a canvas grid's.
    fn tree_rows(
        &self,
        node: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        dock: &WeakEntity<DockArea>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let view = node.view();
        if let Ok(stack) = view.clone().downcast::<StackPanel>() {
            let (axis, children, seams_override) = {
                let stack = stack.read(cx);
                (stack.axis(), stack.panels().to_vec(), stack.seams())
            };
            // The split's own seams over the app-wide toggle. A flip back to
            // the app's value clears the override. An overriding split shows
            // its button at rest.
            let effective = seams_override.unwrap_or_else(settings::seams);
            let seams_stack = stack.clone();
            let seams_button = icon_button(
                if effective {
                    icons::COLUMNS_2
                } else {
                    icons::SQUARE_DASHED
                },
                false,
                cx.listener(move |_, _, _, cx| {
                    let next = !effective;
                    let value = (next != settings::seams()).then_some(next);
                    seams_stack.update(cx, |stack, cx| stack.set_seams(value, cx));
                    cx.notify();
                }),
            );
            let controls = div()
                .flex()
                .flex_row()
                .items_center()
                .child(if seams_override.is_some() {
                    seams_button
                } else {
                    reveal(seams_button)
                })
                .children(self.move_controls(&slot, cx))
                .into_any_element();
            rows.push(chrome_row(
                rows.len(),
                depth,
                match axis {
                    Axis::Horizontal => rox_i18n::t_static("settings-workspace-tree-split-row"),
                    Axis::Vertical => rox_i18n::t_static("settings-workspace-tree-split-column"),
                },
                Some(controls),
            ));
            let len = children.len();
            for (ix, child) in children.into_iter().enumerate() {
                let child_slot = TreeSlot::Stack {
                    stack: stack.clone(),
                    dock: dock.clone(),
                    ix,
                    len,
                };
                self.tree_rows(child, depth + 1, child_slot, dock, rows, cx);
            }
            return;
        }
        if let Ok(tabs) = view.downcast::<TabPanel>() {
            let children = tabs.read(cx).panels().to_vec();
            // A group of one reads as its panel, and inherits the group's slot
            // so its arrows move the group.
            if let [only] = children.as_slice() {
                self.panel_rows(only.clone(), depth, slot, rows, cx);
                return;
            }
            rows.push(chrome_row(
                rows.len(),
                depth,
                rox_i18n::t_static("settings-workspace-tree-tabs"),
                self.move_controls(&slot, cx),
            ));
            let len = children.len();
            for (ix, child) in children.into_iter().enumerate() {
                let child_slot = TreeSlot::Tabs {
                    tabs: tabs.clone(),
                    ix,
                    len,
                };
                self.panel_rows(child, depth + 1, child_slot, rows, cx);
            }
            return;
        }
        self.panel_rows(node, depth, slot, rows, cx);
    }

    fn panel_rows(
        &self,
        panel: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let children = crate::composite::hosted_children(&panel, cx);
        let grids = crate::composite::hosted_docks(&panel, cx);
        rows.push(self.panel_row(rows.len(), panel, depth, slot, cx));
        // A canvas's grids are docks of their own, and their rows keep the
        // move and lock controls, since they are real tab groups.
        for (name, grid) in grids.into_iter().flatten() {
            rows.push(chrome_row(
                rows.len(),
                depth + 1,
                rox_i18n::t!("canvas-tree-grid", name = name),
                None,
            ));
            let root = grid.read(cx).items().view();
            self.tree_rows(root, depth + 2, TreeSlot::Root, &grid.downgrade(), rows, cx);
        }
        if let Some(children) = children {
            for child in children {
                match child {
                    // A host can hold another host.
                    Some(child) => self.panel_rows(child, depth + 1, TreeSlot::Hosted, rows, cx),
                    None => rows.push(chrome_row(
                        rows.len(),
                        depth + 1,
                        rox_i18n::t_static("settings-workspace-tree-empty-slot"),
                        None,
                    )),
                }
            }
        }
    }

    fn panel_row(
        &self,
        ix: usize,
        panel: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let type_name = panel::display_name(panel.panel_name(cx));
        let name: SharedString = match panel.tab_name(cx) {
            Some(custom) => format!("{custom} ({type_name})").into(),
            None => type_name.into(),
        };
        let hosted = matches!(slot, TreeSlot::Hosted);
        let locked = panel.locked(cx);
        let lock_panel = panel.clone();
        div()
            // Named after its place in the tree: same-kind panels carry the
            // same controls.
            .id(ElementId::NamedInteger("tree-row".into(), ix as u64))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_MD)
            .pl(indent(depth))
            .group(TREE_ROW_GROUP)
            .child(div().min_w_0().truncate().child(name))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .children(self.move_controls(&slot, cx))
                    .when(!hosted, |d| {
                        let button = icon_button(
                            if locked {
                                icons::LOCK
                            } else {
                                icons::LOCK_OPEN
                            },
                            false,
                            cx.listener(move |_, _, _, cx| {
                                panel_settings::toggle_locked_for_view(&lock_panel, cx);
                                cx.notify();
                            }),
                        );
                        // A closed lock shows at rest.
                        d.child(if locked { button } else { reveal(button) })
                    })
                    .child(reveal(icon_button(
                        icons::SETTINGS,
                        false,
                        move |_, _, cx| {
                            panel_settings::open_for_view(&panel, cx);
                        },
                    ))),
            )
            .into_any_element()
    }

    fn move_controls(&self, slot: &TreeSlot, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (ix, len) = match slot {
            TreeSlot::Stack { ix, len, .. } | TreeSlot::Tabs { ix, len, .. } => (*ix, *len),
            TreeSlot::Root | TreeSlot::Hosted => return None,
        };
        let lift = self.lift_button(slot, cx);
        let up = self.move_button(slot, icons::ARROW_UP, ix == 0, ix.wrapping_sub(1), cx);
        let down = self.move_button(slot, icons::ARROW_DOWN, ix + 1 >= len, ix + 1, cx);
        Some(
            reveal(div())
                .flex()
                .flex_row()
                .items_center()
                .child(lift)
                .child(up)
                .child(down)
                .into_any_element(),
        )
    }

    /// A tab leaves its group for a group of its own; a split's child moves
    /// into the enclosing split. The root split's children stay put.
    fn lift_button(&self, slot: &TreeSlot, cx: &mut Context<Self>) -> AnyElement {
        match slot {
            TreeSlot::Stack {
                stack, dock, ix, ..
            } => {
                let dock = dock.upgrade().map(|dock| dock.downgrade());
                let inert = dock.is_none() || stack.read(cx).parent().is_none();
                let stack = stack.clone();
                let from = *ix;
                icon_button(
                    icons::ARROW_LEFT,
                    inert,
                    cx.listener(move |this, _, _, cx| {
                        let Some(dock) = dock.clone() else {
                            return;
                        };
                        this.workspace_window
                            .update(cx, |_, window, cx| {
                                stack.update(cx, |stack, cx| {
                                    stack.lift_panel(from, dock, window, cx)
                                });
                            })
                            .ok();
                        cx.notify();
                    }),
                )
                .into_any_element()
            }
            TreeSlot::Tabs { tabs, ix, .. } => {
                let tabs = tabs.clone();
                let from = *ix;
                icon_button(
                    icons::ARROW_LEFT,
                    false,
                    cx.listener(move |this, _, _, cx| {
                        this.workspace_window
                            .update(cx, |_, window, cx| {
                                tabs.update(cx, |tabs, cx| tabs.lift_panel(from, window, cx));
                            })
                            .ok();
                        cx.notify();
                    }),
                )
                .into_any_element()
            }
            TreeSlot::Root | TreeSlot::Hosted => div().into_any_element(),
        }
    }

    /// The ends render inert even though the move APIs ignore out-of-range
    /// indices, so the tree shows where a row can go.
    fn move_button(
        &self,
        slot: &TreeSlot,
        icon: &'static str,
        inert: bool,
        to_ix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match slot {
            TreeSlot::Stack { stack, ix, .. } => {
                let stack = stack.clone();
                let from = *ix;
                icon_button(
                    icon,
                    inert,
                    cx.listener(move |_, _, _, cx| {
                        stack.update(cx, |stack, cx| stack.move_panel(from, to_ix, cx));
                        cx.notify();
                    }),
                )
                .into_any_element()
            }
            TreeSlot::Tabs { tabs, ix, .. } => {
                let tabs = tabs.clone();
                let from = *ix;
                icon_button(
                    icon,
                    inert,
                    cx.listener(move |_, _, _, cx| {
                        tabs.update(cx, |tabs, cx| tabs.move_panel(from, to_ix, cx));
                        cx.notify();
                    }),
                )
                .into_any_element()
            }
            TreeSlot::Root | TreeSlot::Hosted => div().into_any_element(),
        }
    }
}

impl Render for CompositionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-workspace-composition-hint"));
        let body = div().flex().flex_col().gap(tokens::SPACE_XS);

        // The convert row leads: a long tree would push it out of sight.
        match self.workspace.upgrade() {
            Some(workspace) => body
                .child(self.convert_row(&workspace, cx))
                .child(hint)
                .child(self.tree(&workspace, cx)),
            None => body.child(hint).child(
                div()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-workspace-composition-closed")),
            ),
        }
    }
}

/// The closed lock skips this in `panel_row`: it shows state worth seeing at
/// rest.
fn reveal<E: Styled + InteractiveElement>(control: E) -> E {
    control
        .opacity(0.)
        .group_hover(TREE_ROW_GROUP, |style| style.opacity(1.))
}

fn indent(depth: usize) -> Pixels {
    px(14. * depth as f32)
}

/// The dock root and hosted children aren't movable here; the composite orders
/// its own.
#[derive(Clone)]
enum TreeSlot {
    Root,
    Stack {
        stack: Entity<StackPanel>,
        /// The area the stack is in, which a lift hands the moved panel.
        dock: WeakEntity<DockArea>,
        ix: usize,
        len: usize,
    },
    Tabs {
        tabs: Entity<TabPanel>,
        ix: usize,
        len: usize,
    },
    Hosted,
}

fn chrome_row(
    ix: usize,
    depth: usize,
    label: impl Into<SharedString>,
    controls: Option<AnyElement>,
) -> AnyElement {
    div()
        // Named after its place in the tree, since every row carries the same
        // arrows.
        .id(ElementId::NamedInteger("tree-row".into(), ix as u64))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .pl(indent(depth))
        .group(TREE_ROW_GROUP)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label.into())
        .when_some(controls, |d, controls| d.child(controls))
        .into_any_element()
}
