//! The menubar: the dropdown menus, their layout and workspace flyout
//! submenus, and menu action dispatch. Split out of the workspace shell it
//! renders into; it reaches back into the same private state, so these are
//! `impl Workspace` methods in a child module.

use super::*;

impl Workspace {
    pub(crate) fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::NewWindow => crate::open_workspace(cx),
            MenuAction::EmptyWindow => crate::open_workspace_with(WorkspaceStart::Empty, cx),
            MenuAction::OpenSettings => crate::settings::window::open(
                self.state.clone(),
                cx.entity().downgrade(),
                window.window_handle(),
                self.dock.clone(),
                cx,
            ),
            MenuAction::OpenPanel(def) => {
                let panel = (def.build)(&self.state, cx.entity().downgrade(), window, cx);
                match def.placement {
                    PanelPlacement::Center => self.add_center(panel, window, cx),
                    PanelPlacement::Bottom => self.add_bottom(panel, window, cx),
                    PanelPlacement::Top => self.add_top(panel, window, cx),
                }
            }
            MenuAction::OpenStats => crate::stats_window::open(self.state.clone(), cx),
            MenuAction::OpenWelcome => crate::startup::welcome_window::open(self.state.clone(), cx),
            MenuAction::ToggleMenubar => {
                let on = !settings::hide_menubar();
                settings::set_hide_menubar(on, cx);
                Settings::update(move |s| s.hide_menubar = on);
            }
            MenuAction::ToggleDecorations => {
                let on = !settings::os_decorations();
                settings::set_os_decorations(on);
                Settings::update(move |s| s.os_decorations = on);
                apply_decorations(cx);
            }
            MenuAction::ImportWorkspace => self.import_workspace(window, cx),
            MenuAction::ToggleQuitToTray => {
                let on = !settings::quit_to_tray();
                settings::set_quit_to_tray(on);
                Settings::update(move |s| s.quit_to_tray = on);
                tray::sync(cx);
            }
            MenuAction::CloseWindow => {
                // Deferred out of this update: the teardown persists the
                // layout and dumps every panel, this workspace included, and
                // a read inside its own update panics. Same teardown the OS
                // close button and Window Controls close button run, so
                // shutting the last workspace window quits; a popped-out menu
                // panel isn't a workspace window, so it just closes.
                let ws = cx.entity();
                window.defer(cx, move |window, cx| {
                    if is_workspace_window(window, cx) {
                        close_workspace_window(Some(ws), window, cx);
                    }
                    window.remove_window();
                });
            }
            MenuAction::Quit => {
                // Same as the Quit action: quitting bypasses the window close
                // hook, so dump the layout and frame here or a pending
                // debounce and any window move since the last save are lost.
                self.persist(window, cx);
                cx.quit();
            }
        }
    }

    fn menu_button(
        &self,
        index: usize,
        menu: &'static Menu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_menu == Some(index);
        div()
            .relative()
            .h_full()
            .px(tokens::SPACE_MD)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_active()))
            .hover(|d| d.bg(palette::bg_menu_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = if this.open_menu == Some(index) {
                        None
                    } else {
                        Some(index)
                    };
                    this.open_submenu = None;
                    cx.notify();
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    cx.notify();
                }))
            })
            .child(menu.label)
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }
    /// The menubar row: the mini toggle, the menus, and the status side.
    /// One builder so the docked row and the alt-revealed overlay stay
    /// the same bar.
    pub(crate) fn menubar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(MENU_BAR_H))
            .flex_none()
            .bg(palette::bg_menubar())
            .border_b_1()
            .border_color(palette::border())
            .children(self.mini_button(cx))
            .children(
                MENUS
                    .iter()
                    .enumerate()
                    .map(|(i, menu)| self.menu_button(i, menu, cx)),
            )
            .child(div().flex_1())
            .child(self.library_status(cx))
    }

    /// The menubar's right side: the catalog status line, a badge while a
    /// scan or load runs, a rescan button once a folder is known, and an
    /// abort button while a scan runs.
    fn library_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (busy, status, can_rescan, scanning) = {
            let library = self.state.library.read(cx);
            (
                library.busy(),
                library.status(),
                library.can_rescan(),
                library.scanning(),
            )
        };
        let idle = busy.is_none();
        // Status text leftmost so its width changes grow into the empty
        // middle of the bar; the badge and buttons keep their spot at the
        // right edge.
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .when(!status.is_empty(), |d| {
                d.child(
                    div()
                        .max_w(px(480.))
                        .truncate()
                        .text_color(palette::text_muted())
                        // While scanning the status is the full path of the
                        // file under the cursor: smaller text.
                        .when(scanning, |d| d.text_xs())
                        .child(status),
                )
            })
            .when_some(busy, |d, label| {
                // Tabular digits, so the count ticking up never changes
                // the badge width within a digit count.
                let mut badge = div()
                    .px(tokens::SPACE_SM)
                    .py(px(2.))
                    .rounded_full()
                    .bg(palette::accent())
                    .text_xs()
                    .text_color(palette::text_on_accent());
                badge
                    .text_style()
                    .get_or_insert_with(Default::default)
                    .font_features = Some(FontFeatures(Arc::new(vec![("tnum".into(), 1)])));
                d.child(badge.child(label))
            })
            .when(can_rescan && idle, |d| {
                d.child(panel::icon_control_sized(
                    icons::REFRESH_CW,
                    px(12.),
                    palette::text_muted(),
                    |this: &mut Workspace, cx| {
                        this.state
                            .library
                            .update(cx, |library, cx| library.rescan(cx));
                    },
                    cx,
                ))
            })
            .when(scanning, |d| {
                d.child(panel::icon_control_sized(
                    icons::CLOSE,
                    px(12.),
                    palette::text_muted(),
                    |this: &mut Workspace, cx| {
                        this.state
                            .library
                            .update(cx, |library, cx| library.abort_scan(cx));
                    },
                    cx,
                ))
            })
    }

    fn dropdown(&self, menu: &'static Menu, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .top(px(MENU_BAR_H))
            .min_w(px(180.))
            .flex()
            .flex_col()
            .py(tokens::SPACE_XS)
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .children(menu.entries.iter().enumerate().map(|(i, entry)| {
                match entry {
                    MenuEntry::Item(item) => self
                        .action_item(*item, cx)
                        .id(("menu-entry", i))
                        // Sliding onto a plain item retracts a flyout a
                        // sibling submenu left open.
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered && this.open_submenu.is_some() {
                                this.open_submenu = None;
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
                                self.action_item(panel_menu_item(def), cx)
                                    .id(("panel-entry", j))
                                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                        if *hovered && this.open_submenu.is_some() {
                                            this.open_submenu = None;
                                            cx.notify();
                                        }
                                    }))
                            }))
                            .into_any_element(),
                        Some((label, icon)) => self
                            .submenu_row(i, label, icon, section.panels, cx)
                            .into_any_element(),
                    },
                    MenuEntry::LayoutsSubmenu {
                        label,
                        icon,
                        target,
                        with_new,
                    } => self
                        .layouts_submenu_row(i, label, icon, *target, *with_new, cx)
                        .into_any_element(),
                    MenuEntry::WorkspacesSubmenu {
                        label,
                        icon,
                        target,
                        with_new,
                    } => self
                        .workspaces_submenu_row(i, label, icon, *target, *with_new, cx)
                        .into_any_element(),
                }
            }))
    }

    /// A dropdown row that runs an action and closes the menu. The caller
    /// chains its hover behavior, which differs between the top level and a
    /// flyout.
    fn action_item(&self, item: MenuItem, cx: &mut Context<Self>) -> Div {
        let action = item.action;
        // The static menu table can't carry state, so the toggle row reads
        // its check live.
        let checked = match action {
            MenuAction::ToggleMenubar => settings::hide_menubar(),
            MenuAction::ToggleDecorations => settings::os_decorations(),
            MenuAction::ToggleQuitToTray => settings::quit_to_tray(),
            _ => false,
        };
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    cx.notify();
                    this.run(action, window, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                svg()
                    .path(item.icon)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child(item.label)
            // The trailing slot: the row's keybinding, or the check while
            // a toggle row is on. The spacer pushes it to the right edge.
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

    /// A dropdown row that flies its items out to the side while hovered.
    /// The flyout stays open until another entry is hovered or the menu
    /// closes, so the pointer can cross the gap without losing it.
    fn submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        panels: &'static [PanelDef],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        div()
            .id(("menu-entry", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_submenu = Some(index);
                    cx.notify();
                }
            }))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        svg()
                            .path(icon)
                            .size_3p5()
                            .text_color(palette::text_muted()),
                    )
                    .child(label),
            )
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                d.child(
                    // Top offset backs out the parent's padding and the
                    // dropdown border so the first item lines up with the
                    // parent row.
                    div()
                        .absolute()
                        .left_full()
                        .top(px(-5.))
                        .min_w(px(160.))
                        .flex()
                        .flex_col()
                        .py(tokens::SPACE_XS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .occlude()
                        .children(
                            panels
                                .iter()
                                .map(|def| self.action_item(panel_menu_item(def), cx)),
                        ),
                )
            })
    }

    /// The layout-presets flyout: like [`Workspace::submenu_row`] but its
    /// items are the saved and shipped presets, read when it opens, each
    /// doing the flyout's `target` with that preset. With `with_new` the
    /// list leads with a "New..." row that opens the save dialog, so the
    /// Save Layout flyout can start a fresh preset as well as overwrite.
    fn layouts_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: LayoutTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        div()
            .id(("menu-entry", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_submenu = Some(index);
                    cx.notify();
                }
            }))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        svg()
                            .path(icon)
                            .size_3p5()
                            .text_color(palette::text_muted()),
                    )
                    .child(label),
            )
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                // Read the presets only once the flyout opens, not on every
                // parent-menu paint.
                let presets = crate::settings::layouts::all(&Settings::load());
                let mut flyout = div()
                    .absolute()
                    .left_full()
                    .top(px(-5.))
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .py(tokens::SPACE_XS)
                    .bg(palette::bg_menu_opaque())
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .occlude();
                if with_new {
                    flyout = flyout.child(self.save_new_item(cx));
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
                            .map(|preset| self.layout_item(preset.name, target, cx)),
                    );
                }
                d.child(flyout)
            })
    }

    /// The Save flyout's leading row: opens the save dialog for a fresh
    /// preset, closing the menu first like every other flyout row.
    fn save_new_item(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    this.open_save_dialog(window, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                svg()
                    .path(icons::PLUS)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child("New...")
    }

    /// A preset row in a layouts flyout: closes the menu, then does the
    /// flyout's thing with the named preset - open a window, overwrite it
    /// with the current arrangement, or apply it here behind a confirm.
    fn layout_item(
        &self,
        name: String,
        target: LayoutTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = SharedString::from(name.clone());
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    cx.notify();
                    this.run_layout(name.clone(), target, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                svg()
                    .path(icons::LAYOUT_DASHBOARD)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child(label)
    }

    /// A workspaces flyout: like [`Workspace::layouts_submenu_row`] but its
    /// items are the saved and shipped workspaces, read when it opens, each
    /// doing the flyout's `target` with that bundle behind a confirm. With
    /// `with_new` the list leads with a "New..." row that opens the save
    /// dialog, so the Save Workspace flyout can start a fresh bundle as well
    /// as overwrite.
    fn workspaces_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: WorkspaceTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        div()
            .id(("menu-entry", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_submenu = Some(index);
                    cx.notify();
                }
            }))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        svg()
                            .path(icon)
                            .size_3p5()
                            .text_color(palette::text_muted()),
                    )
                    .child(label),
            )
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                // Read the workspaces only once the flyout opens, not on every
                // parent-menu paint.
                let entries = crate::workspaces::all(&Settings::load());
                let mut flyout = div()
                    .absolute()
                    .left_full()
                    .top(px(-5.))
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .py(tokens::SPACE_XS)
                    .bg(palette::bg_menu_opaque())
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .occlude();
                if with_new {
                    flyout = flyout.child(self.save_new_workspace_item(cx));
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
                        self.workspace_item(entry.bundle.name, entry.builtin, target, cx)
                    }));
                }
                d.child(flyout)
            })
    }

    /// The Save Workspace flyout's leading row: opens the save dialog for a
    /// fresh bundle, closing the menu first like every other flyout row.
    fn save_new_workspace_item(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    this.open_save_workspace_dialog(window, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                svg()
                    .path(icons::PLUS)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child("New...")
    }

    /// A workspace row in a workspaces flyout: closes the menu, then stages
    /// the flyout's confirm with the named bundle. A shipped bundle trails a
    /// muted tag to tell it from the user's own.
    fn workspace_item(
        &self,
        name: String,
        builtin: bool,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = SharedString::from(name.clone());
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    this.run_workspace(name.clone(), target, cx);
                }),
            )
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                svg()
                    .path(icons::GALLERY)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
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
}
