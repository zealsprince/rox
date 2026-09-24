//! The menu panel: the menubar's menus behind one button, off the same MENUS
//! table, so a layout keeps them reachable with the menubar hidden. Hand-drawn
//! like the menubar rather than with gpui-component's PopupMenu, so the two
//! read the same.

use gpui::{
    AnyElement, App, Bounds, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, MouseDownEvent, Pixels, Point, SharedString, WeakEntity, Window, anchored, canvas,
    deferred, div, point, prelude::*, px, svg,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::panel_catalog::PanelDef;
use crate::workspace::menubar::{
    NavRow, NavSlot, menu_entry_rows, nav_lit, nav_row_at, step_index, subgroup_rows, submenu_rows,
};
use crate::workspace::{
    LayoutTarget, MENUS, Menu, MenuAction, MenuEntry, MenuItem, PanelTarget, Workspace,
    WorkspaceTarget, flyout_leftward, flyout_side, menu_item_display, menu_section,
    panel_menu_item, section_shows, shortcut_for, signal_marked,
};
use rox_core::settings::{self, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::{Align, align_row, justify};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MenuConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
}

pub struct MenuPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: MenuConfig,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    open_at: Option<Point<Pixels>>,
    open_top: Option<usize>,
    open_sub: Option<usize>,
    /// The third level: the Window menu's panel picker groups.
    open_subgroup: Option<usize>,
    /// Painted bounds of the open surfaces, read next frame to pick each
    /// flyout's side.
    menu_surfaces: [Option<Bounds<Pixels>>; 3],
    button_bounds: Option<Bounds<Pixels>>,
    /// Held apart from `open_top` so the arrows walk the list without flying
    /// every menu out. None while the pointer drives.
    nav_top: Option<usize>,
    /// Some once the keyboard has stepped into the dropdown.
    nav_slot: Option<NavSlot>,
    nav_sub: Option<usize>,
    nav_group: Option<usize>,
    menu_viewport_w: Pixels,
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
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            open_at: None,
            open_top: None,
            open_sub: None,
            open_subgroup: None,
            menu_surfaces: [None; 3],
            button_bounds: None,
            nav_top: None,
            nav_slot: None,
            nav_sub: None,
            nav_group: None,
            menu_viewport_w: Pixels::ZERO,
        }
    }

    // `+ use<>` here and below: a 2024 `impl Trait` return captures every
    // lifetime in scope, and these must not borrow `cx`.
    fn menu_surface_capture(
        &self,
        level: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity();
        canvas(
            move |bounds, window, cx| {
                let viewport_w = window.viewport_size().width;
                view.update(cx, |this, _| {
                    this.menu_surfaces[level] = Some(bounds);
                    this.menu_viewport_w = viewport_w;
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    fn flyout_left(&self, level: usize) -> bool {
        flyout_leftward(&self.menu_surfaces, level, self.menu_viewport_w)
    }

    fn open_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_at = Some(position);
        self.open_top = None;
        self.open_sub = None;
        self.open_subgroup = None;
        self.nav_top = None;
        self.nav_slot = None;
        self.nav_sub = None;
        self.nav_group = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn open_from_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let at = self
            .button_bounds
            .map(|bounds| point(bounds.origin.x, bounds.bottom()))
            .unwrap_or_default();
        self.open_menu(at, window, cx);
        self.nav_top = Some(0);
    }

    fn button_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        canvas(
            move |bounds, _, cx| {
                view.update(cx, |this, _| this.button_bounds = Some(bounds));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open_at = None;
        self.open_top = None;
        self.open_sub = None;
        self.open_subgroup = None;
        self.nav_top = None;
        self.nav_slot = None;
        self.nav_sub = None;
        self.nav_group = None;
        cx.notify();
    }

    fn open_top_at(&mut self, index: usize) {
        self.open_top = Some(index);
        self.nav_top = Some(index);
        self.nav_slot = None;
        self.open_flyout(None);
    }

    /// The cursor goes too: a hover elsewhere must not leave a highlight in a
    /// list it left.
    fn open_flyout(&mut self, index: Option<usize>) {
        self.open_sub = index;
        self.nav_sub = None;
        self.open_subgroup = None;
        self.nav_group = None;
    }

    /// Reports whether the key was used. An open menu owns the keyboard, so a
    /// stray key must not land underneath. The key context hands Space and the
    /// arrows over (see `keymap::PLAYBACK`), since bindings beat listeners.
    fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return false;
        }
        let key = event.keystroke.key.as_str();
        if self.open_at.is_none() {
            // Down opens it too, like any dropdown.
            if matches!(key, "enter" | "space" | "down") {
                self.open_from_key(window, cx);
                return true;
            }
            return false;
        }
        match key {
            "escape" => self.nav_escape(cx),
            "up" => self.nav_step(-1, cx),
            "down" => self.nav_step(1, cx),
            "left" => self.nav_out(cx),
            "right" => self.nav_in(cx),
            "enter" | "space" => self.nav_enter(window, cx),
            _ => {}
        }
        true
    }

    fn nav_escape(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.nav_group = None;
        } else if self.open_sub.is_some() {
            self.open_flyout(None);
        } else if self.open_top.is_some() {
            self.open_top = None;
            self.nav_slot = None;
        } else {
            self.close(cx);
            return;
        }
        cx.notify();
    }

    fn nav_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.nav_group = step_index(self.nav_group, delta, self.group_rows().len());
        } else if self.open_sub.is_some() {
            self.nav_sub = step_index(self.nav_sub, delta, self.sub_rows().len());
        } else if self.nav_slot.is_some() {
            let rows = self.menu_rows();
            let at = self
                .nav_slot
                .and_then(|slot| rows.iter().position(|(row, _)| *row == slot));
            self.nav_slot = step_index(at, delta, rows.len()).map(|i| rows[i].0);
        } else if let Some(top) = step_index(self.nav_top, delta, MENUS.len()) {
            match self.open_top {
                Some(_) => self.open_top_at(top),
                None => self.nav_top = Some(top),
            }
        }
        cx.notify();
    }

    fn nav_out(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.nav_group = None;
        } else if self.open_sub.is_some() {
            self.open_flyout(None);
        } else if self.nav_slot.is_some() {
            self.nav_slot = None;
        } else {
            self.close(cx);
            return;
        }
        cx.notify();
    }

    fn nav_in(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            return;
        }
        if self.open_sub.is_some() {
            if let Some(NavRow::Open(group)) = nav_row_at(self.sub_rows(), self.nav_sub) {
                self.open_subgroup = Some(group);
                self.nav_group = (!self.group_rows().is_empty()).then_some(0);
                cx.notify();
            }
            return;
        }
        if self.nav_slot.is_some() {
            if let Some(NavRow::Open(entry)) = self.current_row() {
                self.open_flyout(Some(entry));
                self.nav_sub = (!self.sub_rows().is_empty()).then_some(0);
                cx.notify();
            }
            return;
        }
        self.open_top_at(self.nav_top.unwrap_or(0));
        self.nav_slot = self.menu_rows().first().map(|(slot, _)| *slot);
        cx.notify();
    }

    fn nav_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = if self.open_subgroup.is_some() {
            nav_row_at(self.group_rows(), self.nav_group)
        } else if self.open_sub.is_some() {
            nav_row_at(self.sub_rows(), self.nav_sub)
        } else if self.nav_slot.is_some() {
            self.current_row()
        } else {
            self.nav_in(cx);
            return;
        };
        match row {
            Some(NavRow::Run(run)) => {
                self.close(cx);
                let Some(ws) = self.workspace.upgrade() else {
                    return;
                };
                ws.update(cx, |ws, cx| ws.run_nav(run, window, cx));
            }
            Some(NavRow::Open(_)) => self.nav_in(cx),
            None => {}
        }
    }

    fn current_row(&self) -> Option<NavRow> {
        let slot = self.nav_slot?;
        self.menu_rows()
            .into_iter()
            .find(|(row, _)| *row == slot)
            .map(|(_, row)| row)
    }

    fn menu_rows(&self) -> Vec<(NavSlot, NavRow)> {
        self.open_top.map(menu_entry_rows).unwrap_or_default()
    }

    fn sub_rows(&self) -> Vec<NavRow> {
        match (self.open_top, self.open_sub) {
            (Some(menu), Some(entry)) => submenu_rows(menu, entry),
            _ => Vec::new(),
        }
    }

    fn group_rows(&self) -> Vec<NavRow> {
        self.open_subgroup.map(subgroup_rows).unwrap_or_default()
    }

    fn nav_on(&self, entry: usize, row: Option<usize>) -> bool {
        self.nav_slot == Some((entry, row))
    }

    fn nav_on_sub(&self, row: usize) -> bool {
        self.nav_sub == Some(row)
    }

    fn nav_on_group(&self, group: usize) -> bool {
        matches!(
            nav_row_at(self.sub_rows(), self.nav_sub),
            Some(NavRow::Open(open)) if open == group
        )
    }

    fn nav_in_group(&self, group: usize, row: usize) -> bool {
        self.open_subgroup == Some(group) && self.nav_group == Some(row)
    }

    fn root_menu(&self, cx: &mut Context<Self>) -> Div {
        dropdown(px(160.))
            .child(self.menu_surface_capture(0, cx))
            .children(
                MENUS
                    .iter()
                    .enumerate()
                    .map(|(i, menu)| self.top_row(i, menu, cx)),
            )
    }

    fn top_row(
        &self,
        index: usize,
        menu: &'static Menu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_top == Some(index);
        row()
            .id(("menu-top", index))
            .relative()
            .justify_between()
            .when(open || self.nav_top == Some(index), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_top != Some(index) {
                    this.open_top_at(index);
                    cx.notify();
                }
            }))
            .child(rox_i18n::t!(menu.label))
            .child(chevron())
            .when(open, |d| {
                d.child(flyout_side(self.menu_flyout(menu, cx), self.flyout_left(0)).top(px(-5.)))
            })
    }

    fn menu_flyout(&self, menu: &'static Menu, cx: &mut Context<Self>) -> Div {
        dropdown(px(180.))
            .absolute()
            .child(self.menu_surface_capture(1, cx))
            .children(menu.entries.iter().enumerate().map(|(i, entry)| {
                match entry {
                    MenuEntry::Item(item) => self
                        .action_row(*item, cx)
                        .id(("menu-entry", i))
                        .when(self.nav_on(i, None), nav_lit)
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered && this.open_sub.is_some() {
                                this.open_flyout(None);
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    MenuEntry::Section(label) => menu_section(label).into_any_element(),
                    MenuEntry::Panels(section) if !section_shows(section) => {
                        div().into_any_element()
                    }
                    MenuEntry::Panels(section) => match section.group {
                        None => div()
                            .flex()
                            .flex_col()
                            .children(section.panels.iter().enumerate().map(|(j, def)| {
                                self.action_row(panel_menu_item(def), cx)
                                    .id(("panel-entry", j))
                                    .when(self.nav_on(i, Some(j)), nav_lit)
                                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                        if *hovered && this.open_sub.is_some() {
                                            this.open_flyout(None);
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
                    MenuEntry::PresetsSubmenu {
                        label,
                        icon,
                        target,
                    } => self
                        .presets_row(i, label, icon, *target, cx)
                        .into_any_element(),
                    MenuEntry::PanelWindowsSubmenu { label, icon } => self
                        .panel_windows_row(i, label, icon, cx)
                        .into_any_element(),
                }
            }))
    }

    fn action_row(&self, item: MenuItem, cx: &mut Context<Self>) -> Div {
        let action = item.action;
        // The static table can't hold state, so toggle rows read their check
        // live.
        let checked = match action {
            MenuAction::ToggleMenubar => settings::hide_menubar(),
            MenuAction::ToggleDesignMode => settings::design_mode(),
            MenuAction::ToggleDecorations => settings::os_decorations(),
            MenuAction::ToggleQuitToTray => settings::quit_to_tray(),
            MenuAction::ToggleArtTheming => palette::art_theming(),
            _ => false,
        };
        let player = self.state.player.read(cx);
        let (is_playing, ab) = (player.is_playing(), player.ab_state());
        let sleep = player.sleep_remaining().map(|left| left.as_secs());
        let (label, item_icon) = menu_item_display(item, is_playing, ab, sleep);
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
            .child(icon(item_icon))
            .child(label)
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
            .when(signal_marked(action), |d| {
                d.child(div().flex_1().min_w(px(24.))).child(
                    svg()
                        .path(icons::AUDIO_WAVEFORM)
                        .size_3()
                        .text_color(palette::text_faint()),
                )
            })
    }

    fn group_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        panels: &'static [PanelDef],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-group", index))
            .relative()
            .justify_between()
            .when(open || self.nav_on(index, None), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_flyout(Some(index));
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                d.child(
                    flyout_side(dropdown(px(160.)).absolute(), self.flyout_left(1))
                        .top(px(-5.))
                        .children(panels.iter().enumerate().map(|(row, def)| {
                            self.action_row(panel_menu_item(def), cx)
                                .when(self.nav_on_sub(row), nav_lit)
                        })),
                )
            })
    }

    fn layouts_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        target: LayoutTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-layouts", index))
            .relative()
            .justify_between()
            .when(open || self.nav_on(index, None), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_flyout(Some(index));
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                let presets = rox_core::settings::layouts::all(&Settings::load());
                let mut flyout =
                    flyout_side(dropdown(px(180.)).absolute(), self.flyout_left(1)).top(px(-5.));
                if with_new {
                    flyout = flyout.child(self.new_row(cx).when(self.nav_on_sub(0), nav_lit));
                }
                if presets.is_empty() {
                    // The Save flyout still has its New row.
                    if !with_new {
                        flyout = flyout.child(
                            div()
                                .px(tokens::SPACE_MD)
                                .py(tokens::SPACE_XS)
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("menu-panel-no-layouts")),
                        );
                    }
                } else {
                    flyout =
                        flyout.children(presets.into_iter().enumerate().map(|(row, preset)| {
                            self.preset_row(preset.name, target, cx)
                                .when(self.nav_on_sub(row + usize::from(with_new)), nav_lit)
                        }));
                }
                d.child(flyout)
            })
    }

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
            .child(rox_i18n::t!("menu-panel-new"))
    }

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

    fn presets_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        target: PanelTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_sub == Some(index);
        self.sub_row(index, label, icon_path, open, cx)
            .when(open, |d| {
                let presets = crate::panel_presets::saved();
                let flyout =
                    flyout_side(dropdown(px(180.)).absolute(), self.flyout_left(1)).top(px(-5.));
                d.child(if presets.is_empty() {
                    flyout.child(
                        div()
                            .px(tokens::SPACE_MD)
                            .py(tokens::SPACE_XS)
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("menu-panel-no-presets")),
                    )
                } else {
                    flyout.children(presets.into_iter().enumerate().map(|(row, preset)| {
                        self.preset_panel_row(preset, target, cx)
                            .when(self.nav_on_sub(row), nav_lit)
                    }))
                })
            })
    }

    fn panel_windows_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_sub == Some(index);
        self.sub_row(index, label, icon_path, open, cx)
            .when(open, |d| {
                let presets = crate::panel_presets::saved();
                // Captures its bounds for the group flyouts' side decision.
                let mut flyout = flyout_side(dropdown(px(180.)).absolute(), self.flyout_left(1))
                    .top(px(-5.))
                    .child(self.menu_surface_capture(2, cx));
                // Presets take group 0 when present, so the catalog's groups
                // start one along.
                if !presets.is_empty() {
                    let rows = presets
                        .into_iter()
                        .enumerate()
                        .map(|(row, preset)| {
                            self.preset_panel_row(preset, PanelTarget::NewWindow, cx)
                                .when(self.nav_in_group(0, row), nav_lit)
                        })
                        .collect();
                    flyout = flyout.child(self.panel_window_group(
                        0,
                        crate::panel_presets::GROUP_LABEL,
                        crate::panel_presets::GROUP_ICON,
                        rows,
                        cx,
                    ));
                }
                for (i, section) in crate::panel_catalog::sections().enumerate() {
                    let rows = section
                        .panels
                        .iter()
                        .enumerate()
                        .map(|(row, def)| {
                            self.panel_window_row(def, cx)
                                .when(self.nav_in_group(i + 1, row), nav_lit)
                        })
                        .collect::<Vec<_>>();
                    flyout = match section.group {
                        None => flyout.children(rows),
                        Some((label, icon_path)) => {
                            flyout.child(self.panel_window_group(i + 1, label, icon_path, rows, cx))
                        }
                    };
                }
                d.child(flyout)
            })
    }

    fn panel_window_group(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        rows: Vec<Div>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_subgroup == Some(index);
        row()
            .id(("panel-window-group", index))
            .relative()
            .justify_between()
            .when(open || self.nav_on_group(index), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_subgroup != Some(index) {
                    this.open_subgroup = Some(index);
                    this.nav_group = None;
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                d.child(
                    flyout_side(dropdown(px(160.)).absolute(), self.flyout_left(2))
                        .top(px(-5.))
                        .children(rows),
                )
            })
    }

    fn panel_window_row(&self, def: &'static PanelDef, cx: &mut Context<Self>) -> Div {
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    ws.update(cx, |ws, cx| ws.open_panel_window(def, window, cx));
                }),
            )
            .child(icon(def.icon))
            .child(rox_i18n::t!(def.label))
    }

    fn preset_panel_row(
        &self,
        preset: rox_core::settings::PanelPreset,
        target: PanelTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let icon_path = crate::panel_presets::icon_for(&preset);
        let label = SharedString::from(preset.name.clone());
        let name = preset.name;
        row()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.close(cx);
                    let Some(ws) = this.workspace.upgrade() else {
                        return;
                    };
                    let name = name.clone();
                    ws.update(cx, |ws, cx| ws.run_panel_preset(name, target, window, cx));
                }),
            )
            .child(icon(icon_path))
            .child(label)
    }

    fn sub_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        open: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        row()
            .id(("menu-entry", index))
            .relative()
            .justify_between()
            .when(open || self.nav_on(index, None), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_flyout(Some(index));
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
    }

    fn workspaces_row(
        &self,
        index: usize,
        label: &'static str,
        icon_path: &'static str,
        target: WorkspaceTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_sub == Some(index);
        row()
            .id(("menu-workspaces", index))
            .relative()
            .justify_between()
            .when(open || self.nav_on(index, None), nav_lit)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_sub != Some(index) {
                    this.open_flyout(Some(index));
                    cx.notify();
                }
            }))
            .child(label_with_icon(icon_path, label))
            .child(chevron())
            .when(open, |d| {
                // Names come off the filenames, so this never parses a bundle.
                // The Save flyout drops shipped bundles, which can't be
                // overwritten.
                let mut entries = crate::workspaces::all();
                if target == WorkspaceTarget::Overwrite {
                    entries.retain(|entry| !entry.builtin);
                }
                let mut flyout =
                    flyout_side(dropdown(px(180.)).absolute(), self.flyout_left(1)).top(px(-5.));
                if with_new {
                    flyout =
                        flyout.child(self.new_workspace_row(cx).when(self.nav_on_sub(0), nav_lit));
                }
                if entries.is_empty() {
                    // The Save flyout still has its New row.
                    if !with_new {
                        flyout = flyout.child(
                            div()
                                .px(tokens::SPACE_MD)
                                .py(tokens::SPACE_XS)
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("menu-panel-no-workspaces")),
                        );
                    }
                } else {
                    flyout =
                        flyout.children(entries.into_iter().enumerate().map(|(row, entry)| {
                            self.workspace_row(entry.name, entry.title, entry.builtin, target, cx)
                                .when(self.nav_on_sub(row + usize::from(with_new)), nav_lit)
                        }));
                }
                d.child(flyout)
            })
    }

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
            .child(rox_i18n::t!("menu-panel-new"))
    }

    fn workspace_row(
        &self,
        name: String,
        title: gpui::SharedString,
        builtin: bool,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = title;
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
                        .child(rox_i18n::t!("menu-panel-built-in")),
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
                    .relative()
                    .size(px(24.))
                    .rounded(tokens::RADIUS)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.open_menu(event.position, window, cx);
                        }),
                    )
                    .child(self.button_capture(cx))
                    .child(
                        svg()
                            .path(icons::MENU)
                            .size(px(14.))
                            .text_color(palette::text_muted()),
                    ),
            )
            // Only the root defers; the flyouts are plain absolute children,
            // since nesting a defer panics.
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

fn icon(path: &'static str) -> impl IntoElement {
    svg()
        .path(path)
        .size_3p5()
        .text_color(palette::text_muted())
}

fn chevron() -> impl IntoElement {
    svg()
        .path(icons::CHEVRON_RIGHT)
        .size_3()
        .text_color(palette::text_muted())
}

/// `label` is a message key off `MENUS`, resolved here.
fn label_with_icon(icon_path: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(icon(icon_path))
        .child(rox_i18n::t!(label))
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
        // A focus stop, which also puts its tab group on the focus path for the
        // tab-cycle chord.
        let focus = self.focus.clone();
        // `MenuNav` open and `FocusedControl` closed are carve-outs the bare
        // playback chords respect (see `keymap::PLAYBACK`). Without one, space
        // would play a track and the arrows would seek.
        let context = if self.open_at.is_some() {
            "MenuNav"
        } else {
            rox_panel_kit::ui::CONTROL_CONTEXT
        };
        panel::themed(&chrome, || {
            self.body(window, cx)
                .track_focus(&focus)
                .key_context(context)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if this.on_key(event, window, cx) {
                        cx.stop_propagation();
                    }
                }))
        })
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("menu-panel-title"),
        )
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
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(48.), rox_dock::resizable::PANEL_MIN_SIZE),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, workspace, config) = {
                    let panel = this.read(cx);
                    (
                        panel.state.clone(),
                        panel.workspace.clone(),
                        panel.config.clone(),
                    )
                };
                MenuPanel::new(state, workspace, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}
