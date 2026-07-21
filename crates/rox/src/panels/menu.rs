//! The menu panel: the menubar's menus behind one button, nested a level
//! deeper. A layout keeps the Window and Panels trees reachable with the
//! menubar hidden or the OS decorations off; the same MENUS table drives
//! both, so the two never drift. The dropdown is hand-drawn from the app
//! palette and tokens, the menubar's own way, so the panel and the bar
//! read the same instead of one wearing gpui-component's PopupMenu look.
//! Items run through the workspace that registered the panel, which is why
//! the builder carries its handle.

use std::sync::Arc;

use gpui::{
    anchored, deferred, div, prelude::*, px, svg, AnyElement, App, Context, Div, EventEmitter,
    FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point, SharedString, WeakEntity,
    Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::PanelDef;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::settings::{self, Settings};
use crate::workspace::{
    menu_section, panel_menu_item, shortcut_for, LayoutTarget, Menu, MenuAction, MenuEntry,
    MenuItem, Workspace, WorkspaceTarget, MENUS,
};

/// The menu panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MenuConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
}

pub struct MenuPanel {
    state: AppState,
    /// The workspace the menu items drive; an item clicked after it is
    /// gone (a popped-out panel outliving its window) just no-ops.
    workspace: WeakEntity<Workspace>,
    config: MenuConfig,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Where the root menu is pinned, set to the button's click position;
    /// None while the menu is closed.
    open_at: Option<Point<Pixels>>,
    /// Which top menu's flyout is open (index into MENUS).
    open_top: Option<usize>,
    /// Which nested group flyout is open within the open top menu (its
    /// entry index).
    open_sub: Option<usize>,
}

impl MenuPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: MenuConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        MenuPanel {
            state,
            workspace,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
            open_at: None,
            open_top: None,
            open_sub: None,
        }
    }

    fn open_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.open_at = Some(position);
        self.open_top = None;
        self.open_sub = None;
        cx.notify();
    }

    /// Close the whole overlay: the root menu and any open flyout under it.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.open_at = None;
        self.open_top = None;
        self.open_sub = None;
        cx.notify();
    }

    /// The root menu: one submenu row per top menu, each flying its full
    /// content out to the side. Unlike the menubar the top menus stack
    /// vertically here, but the flyouts are the same hand-drawn dropdowns.
    fn root_menu(&self, cx: &mut Context<Self>) -> Div {
        dropdown(px(160.)).children(
            MENUS
                .iter()
                .enumerate()
                .map(|(i, menu)| self.top_row(i, menu, cx)),
        )
    }

    /// A root row for a top menu: flies out that menu's dropdown on hover,
    /// the same open-until-a-sibling-takes-over behavior as the menubar.
    fn top_row(
        &self,
        index: usize,
        menu: &'static Menu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_top == Some(index);
        row()
            .id(("menu-top", index))
            .relative()
            .justify_between()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_top != Some(index) {
                    this.open_top = Some(index);
                    this.open_sub = None;
                    cx.notify();
                }
            }))
            .child(menu.label)
            .child(chevron())
            .when(open, |d| {
                d.child(self.menu_flyout(menu, cx).left_full().top(px(-5.)))
            })
    }

    /// A top menu's full dropdown, the menubar's `dropdown` content: its
    /// rows and section headers, plus the nested group and layouts flyouts.
    fn menu_flyout(&self, menu: &'static Menu, cx: &mut Context<Self>) -> Div {
        dropdown(px(180.))
            .absolute()
            .children(menu.entries.iter().enumerate().map(|(i, entry)| {
                match entry {
                    MenuEntry::Item(item) => self
                        .action_row(*item, cx)
                        .id(("menu-entry", i))
                        // Sliding onto a plain item retracts a flyout a
                        // sibling group left open.
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered && this.open_sub.is_some() {
                                this.open_sub = None;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    MenuEntry::Section(label) => menu_section(label).into_any_element(),
                    MenuEntry::Panels(section) => match section.group {
                        // A bare section is a run of plain rows in place.
                        None => div()
                            .flex()
                            .flex_col()
                            .children(section.panels.iter().enumerate().map(|(j, def)| {
                                self.action_row(panel_menu_item(def), cx)
                                    .id(("panel-entry", j))
                                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                        if *hovered && this.open_sub.is_some() {
                                            this.open_sub = None;
                                            cx.notify();
                                        }
                                    }))
                            }))
                            .into_any_element(),
                        Some((label, icon)) => self
                            .group_row(i, label, icon, section.panels, cx)
                            .into_any_element(),
                    },
                    MenuEntry::LayoutsSubmenu {
                        label,
                        icon,
                        target,
                        with_new,
                    } => self
                        .layouts_row(i, label, icon, *target, *with_new, cx)
                        .into_any_element(),
                    MenuEntry::WorkspacesSubmenu {
                        label,
                        icon,
                        target,
                        with_new,
                    } => self
                        .workspaces_row(i, label, icon, *target, *with_new, cx)
                        .into_any_element(),
                }
            }))
    }

    /// One action row: closes the overlay, then runs its menubar action
    /// through the workspace. Carries its keybinding or a check the way the
    /// menubar's rows do.
    fn action_row(&self, item: MenuItem, cx: &mut Context<Self>) -> Div {
        let action = item.action;
        // The static menu table can't carry state, so the toggle rows read
        // their check live.
        let checked = match action {
            MenuAction::ToggleMenubar => settings::hide_menubar(),
            MenuAction::ToggleDecorations => settings::os_decorations(),
            MenuAction::ToggleQuitToTray => settings::quit_to_tray(),
            _ => false,
        };
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.run(action, window, cx));
                }),
            )
            .child(icon(item.icon))
            .child(item.label)
            // The trailing slot: the row's keybinding, or the check while a
            // toggle row is on. The spacer pushes it to the right edge.
            .when_some(shortcut_for(action), |d, keys| {
                d.child(div().flex_1().min_w(px(24.))).child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(keys),
                )
            })
            .when(checked, |d| {
                d.child(div().flex_1().min_w(px(24.))).child(
                    svg()
                        .path(icons::CHECK)
                        .size_3()
                        .text_color(palette::text_muted()),
                )
            })
    }

    /// A static submenu row (a Panels group): flies its action items out on
    /// hover, staying open until another entry takes over.
    fn group_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        panels: &'static [PanelDef],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-group", index))
            .relative()
            .justify_between()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_sub = Some(index);
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                d.child(
                    dropdown(px(160.))
                        .absolute()
                        .left_full()
                        .top(px(-5.))
                        .children(
                            panels
                                .iter()
                                .map(|def| self.action_row(panel_menu_item(def), cx)),
                        ),
                )
            })
    }

    /// The layouts flyout row: like [`MenuPanel::group_row`] but its items
    /// are the saved and shipped presets, read when it opens, each doing the
    /// flyout's `target`. With `with_new` the list leads with a "New..." row
    /// that opens the save dialog.
    fn layouts_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        target: LayoutTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-layouts", index))
            .relative()
            .justify_between()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_sub = Some(index);
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                // Read the presets only once the flyout opens.
                let presets = crate::layouts::all(&Settings::load());
                let mut flyout = dropdown(px(180.)).absolute().left_full().top(px(-5.));
                if with_new {
                    flyout = flyout.child(self.new_row(cx));
                }
                if presets.is_empty() {
                    // The Save flyout still has its New row, so only the
                    // preset-only flyouts read empty here.
                    if !with_new {
                        flyout = flyout.child(
                            div()
                                .px(tokens::SPACE_MD)
                                .py(tokens::SPACE_XS)
                                .text_color(palette::text_muted())
                                .child("No layouts"),
                        );
                    }
                } else {
                    flyout = flyout.children(
                        presets
                            .into_iter()
                            .map(|preset| self.preset_row(preset.name, target, cx)),
                    );
                }
                d.child(flyout)
            })
    }

    /// The Save flyout's leading row: closes the overlay, then opens the
    /// save dialog for a fresh preset.
    fn new_row(&self, cx: &mut Context<Self>) -> Div {
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.open_save_dialog(window, cx));
                }),
            )
            .child(icon(icons::PLUS))
            .child("New...")
    }

    /// A preset row in a layouts flyout: closes the overlay, then does the
    /// flyout's `target` with the named preset.
    fn preset_row(&self, name: String, target: LayoutTarget, cx: &mut Context<Self>) -> Div {
        let label = SharedString::from(name.clone());
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.run_layout(name.clone(), target, cx));
                }),
            )
            .child(icon(icons::LAYOUT_DASHBOARD))
            .child(label)
    }

    /// A workspaces flyout row: like [`MenuPanel::layouts_row`] but its items
    /// are the saved and shipped workspaces, read when it opens, each doing
    /// the flyout's `target` with that bundle behind a confirm. With
    /// `with_new` the list leads with a "New..." row that opens the save
    /// dialog, so the Save Workspace flyout can start a fresh bundle as well
    /// as overwrite.
    fn workspaces_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        target: WorkspaceTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-workspaces", index))
            .relative()
            .justify_between()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_sub = Some(index);
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                // Read the workspaces only once the flyout opens. The Save
                // flyout can't overwrite shipped bundles, so it drops them,
                // matching the settings window where shipped rows carry no
                // Overwrite.
                let mut entries = crate::workspaces::all(&Settings::load());
                if target == WorkspaceTarget::Overwrite {
                    entries.retain(|entry| !entry.builtin);
                }
                let mut flyout = dropdown(px(180.)).absolute().left_full().top(px(-5.));
                if with_new {
                    flyout = flyout.child(self.new_workspace_row(cx));
                }
                if entries.is_empty() {
                    // The Save flyout still has its New row, so only the
                    // apply flyout reads empty here.
                    if !with_new {
                        flyout = flyout.child(
                            div()
                                .px(tokens::SPACE_MD)
                                .py(tokens::SPACE_XS)
                                .text_color(palette::text_muted())
                                .child("No workspaces"),
                        );
                    }
                } else {
                    flyout = flyout.children(entries.into_iter().map(|entry| {
                        self.workspace_row(entry.bundle.name, entry.builtin, target, cx)
                    }));
                }
                d.child(flyout)
            })
    }

    /// The Save Workspace flyout's leading row: closes the overlay, then
    /// opens the save dialog for a fresh bundle.
    fn new_workspace_row(&self, cx: &mut Context<Self>) -> Div {
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.open_save_workspace_dialog(window, cx));
                }),
            )
            .child(icon(icons::PLUS))
            .child("New...")
    }

    /// A workspace row in a workspaces flyout: closes the overlay, then
    /// stages the flyout's confirm with the named bundle. A shipped bundle
    /// trails a muted tag to tell it from the user's own.
    fn workspace_row(
        &self,
        name: String,
        builtin: bool,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = SharedString::from(name.clone());
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.run_workspace(name.clone(), target, cx));
                }),
            )
            .child(icon(icons::GALLERY))
            .child(label)
            .when(builtin, |d| {
                d.child(div().flex_1().min_w(px(24.))).child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child("Shipped"),
                )
            })
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .child(
                div()
                    .size(px(24.))
                    .rounded(tokens::RADIUS)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.open_menu(event.position, cx);
                        }),
                    )
                    .child(
                        svg()
                            .path(icons::MENU)
                            .size(px(14.))
                            .text_color(palette::text_muted()),
                    ),
            )
            // An occluding layer swallows a click that lands off the menu,
            // closing it; the anchored menu on top occludes its own clicks
            // so rows keep working. Only the root defers, the flyouts under
            // it are plain absolute children (nesting a defer would panic).
            .when_some(self.open_at, |d, position| {
                d.child(
                    deferred(
                        anchored().child(
                            div()
                                .w(window.bounds().size.width)
                                .h(window.bounds().size.height)
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _, cx| this.close(cx)),
                                )
                                .child(
                                    anchored()
                                        .position(position)
                                        .snap_to_window_with_margin(px(8.))
                                        .child(self.root_menu(cx)),
                                ),
                        ),
                    )
                    .with_priority(1),
                )
            })
    }
}

/// A dropdown surface: the menubar's opaque menu card with a border and
/// shadow, occluding the clicks that land on it.
fn dropdown(min_w: Pixels) -> Div {
    div()
        .min_w(min_w)
        .flex()
        .flex_col()
        .py(tokens::SPACE_XS)
        .bg(palette::bg_menu_opaque())
        .border_1()
        .border_color(palette::border_light())
        .shadow_md()
        .occlude()
}

/// A dropdown row's base: padded, hoverable, a horizontal icon-and-label
/// strip. Callers add the click or hover handler and the trailing slot.
fn row() -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .cursor_pointer()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .hover(|d| d.bg(palette::bg_control_hover_opaque()))
}

/// A row's leading icon, muted at the menubar's size.
fn icon(path: &'static str) -> impl IntoElement {
    svg()
        .path(path)
        .size_3p5()
        .text_color(palette::text_muted())
}

/// The trailing chevron on a row that flies out a submenu.
fn chevron() -> impl IntoElement {
    svg()
        .path(icons::CHEVRON_RIGHT)
        .size_3()
        .text_color(palette::text_muted())
}

/// A submenu row's leading group: its icon and label together, so the
/// chevron can sit at the far edge with `justify_between`.
fn label_with_icon(icon_path: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(icon(icon_path))
        .child(label)
}

impl PanelSettings for MenuPanel {
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

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }
}

impl Render for MenuPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl EventEmitter<PanelEvent> for MenuPanel {}

impl Focusable for MenuPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MenuPanel {
    fn panel_name(&self) -> &'static str {
        "menu"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Menu")
    }

    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        self.config
            .chrome
            .title
            .clone()
            .map(gpui::SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        // The one button plus the strip's padding, raised by any user floor.
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(48.), rox_dock::resizable::PANEL_MIN_SIZE),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate takes the config along, like the transport panels'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, workspace, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.workspace.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| MenuPanel::new(state, workspace, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}
