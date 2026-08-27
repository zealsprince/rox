//! The menubar: the dropdown menus, their layout and workspace flyout
//! submenus, and menu action dispatch. Split out of the workspace shell it
//! renders into; it reaches back into the same private state, so these are
//! `impl Workspace` methods in a child module.

use super::*;

use gpui::MouseDownEvent;

/// Where the Application menu's three project links land. The issue form is
/// the chooser rather than a blank issue, so a report arrives on a template.
const ISSUES_URL: &str = "https://github.com/zealsprince/rox/issues/new/choose";
const DISCUSSIONS_URL: &str = "https://github.com/zealsprince/rox/discussions";
const CHAT_URL: &str = "https://hivecom.net/chat?channel=rox";

impl Workspace {
    pub(crate) fn run(&mut self, action: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            MenuAction::NewWindow => crate::open_workspace(cx),
            MenuAction::EmptyWindow => crate::open_workspace_with(WorkspaceStart::Empty, cx),
            MenuAction::TogglePlayback => {
                self.state
                    .player
                    .update(cx, |player, _| player.toggle_pause());
            }
            MenuAction::Stop => {
                self.state.player.update(cx, |player, cx| player.stop(cx));
            }
            MenuAction::Next => {
                self.state.player.update(cx, |player, cx| player.next(cx));
            }
            MenuAction::Previous => {
                self.state.player.update(cx, |player, _| player.prev());
            }
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
            MenuAction::OpenConsole => crate::console_window::open(cx),
            MenuAction::OpenTasks => crate::tasks_window::open(cx),
            MenuAction::OpenEqualizer => crate::eq_window::open(cx),
            MenuAction::OpenSignals => crate::signals_window::open(cx),
            MenuAction::OpenWelcome => crate::startup::welcome_window::open(self.state.clone(), cx),
            MenuAction::OpenAbout => crate::startup::about_window::open(self.state.clone(), cx),
            MenuAction::ReportIssue => cx.open_url(ISSUES_URL),
            MenuAction::OpenDiscussions => cx.open_url(DISCUSSIONS_URL),
            MenuAction::OpenChat => cx.open_url(CHAT_URL),
            MenuAction::ToggleMenubar => {
                let on = !settings::hide_menubar();
                settings::set_hide_menubar(on, cx);
                Settings::update(move |s| s.look.bundle.appearance.hide_menubar = on);
                native_menu::rebuild(cx);
            }
            MenuAction::ToggleDesignMode => toggle_design_mode(cx),
            MenuAction::ToggleDecorations => {
                let on = !settings::os_decorations();
                settings::set_os_decorations(on);
                Settings::update(move |s| s.look.bundle.appearance.os_decorations = on);
                apply_decorations(cx);
                native_menu::rebuild(cx);
            }
            MenuAction::ToggleArtTheming => {
                let on = !palette::art_theming();
                palette::set_art_theming(on, cx);
                Settings::update(move |s| s.look.bundle.appearance.art_theming = on);
                native_menu::rebuild(cx);
            }
            MenuAction::TogglePostShader => {
                crate::workspace::toggle_post_shader(cx);
                native_menu::rebuild(cx);
            }
            MenuAction::ImportWorkspace => self.import_workspace(window, cx),
            MenuAction::ToggleQuitToTray => {
                let on = !settings::quit_to_tray();
                settings::set_quit_to_tray(on);
                Settings::update(move |s| s.quit_to_tray = on);
                tray::sync(cx);
                native_menu::rebuild(cx);
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

    /// The modifiers changed, which is where the pin comes from: a
    /// double-tap of Alt leaves a hidden menubar up.
    pub(crate) fn note_modifiers(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        if self
            .alt_tap
            .note(modifiers, Instant::now(), self.pointer_down)
        {
            self.menubar_pinned = !self.menubar_pinned;
            self.menubar_touched = false;
            cx.notify();
        }
    }

    /// Something landed under the held Alt, so it's a chord or a drag rather
    /// than a tap, and the pair it might have completed is off too.
    pub(crate) fn cancel_alt_tap(&mut self) {
        self.alt_tap.cancel();
    }

    /// Drop a pinned menubar. Reports whether there was one, so escape can
    /// stop at the bar instead of falling through to what it backs out of
    /// next.
    pub(crate) fn unpin_menubar(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.menubar_pinned {
            return false;
        }
        self.menubar_pinned = false;
        self.menubar_touched = false;
        cx.notify();
        true
    }

    /// The pointer crossing the pinned bar's edge. Entering arms the pin,
    /// leaving drops it, so the bar clears itself once it's been used. A
    /// leave with a dropdown open is the pointer walking into the dropdown,
    /// which hangs below the bar's bounds, so the pin holds through it.
    pub(crate) fn note_menubar_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if !self.menubar_pinned {
            return;
        }
        if hovered {
            self.menubar_touched = true;
        } else if self.menubar_touched && self.open_menu.is_none() {
            self.unpin_menubar(cx);
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
                    this.open_subgroup = None;
                    cx.notify();
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.open_menu = None;
                    this.open_submenu = None;
                    this.open_subgroup = None;
                    cx.notify();
                }))
            })
            .child(rox_i18n::t!(menu.label))
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }
    /// The menubar row: the mini toggle, the menus, and the status side.
    /// One builder so the docked row and the alt-revealed overlay stay
    /// the same bar.
    pub(crate) fn menubar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        // On macOS the menus live in the system bar, so this row keeps only
        // what the system bar has no place for: the mini toggle, the drag
        // handle, and the library status.
        let native_menus = cfg!(target_os = "macos");
        div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(MENU_BAR_H))
            .flex_none()
            .bg(palette::bg_menubar())
            .border_b_1()
            .border_color(palette::border())
            .children(self.traffic_lights(window, cx))
            .children(self.mini_button(cx))
            .when(!native_menus, |d| {
                d.children(
                    MENUS
                        .iter()
                        .enumerate()
                        .map(|(i, menu)| self.menu_button(i, menu, cx)),
                )
            })
            // The empty middle is a drag handle, so a decorations-off
            // window still moves by its menu bar. The move is the
            // compositor's, same as the drag anchor panel.
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
            )
            .child(self.library_status(cx))
    }

    /// The macOS window buttons at the menubar's left edge, when this window
    /// draws its own chrome. With OS decorations on, the real ones are up in
    /// the native titlebar and a second set here would just be a copy; off
    /// every other platform there are no traffic lights to match.
    fn traffic_lights(&self, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !cfg!(target_os = "macos") || settings::os_decorations() {
            return None;
        }
        // Close runs the menu's own Close, the teardown that persists the
        // layout and only quits from the last workspace window.
        let close = cx.listener(|this: &mut Workspace, _: &MouseDownEvent, window, cx| {
            this.run(MenuAction::CloseWindow, window, cx);
        });
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h_full()
                .flex_none()
                .gap(tokens::SPACE_SM)
                .px(tokens::SPACE_MD)
                .children(rox_panel_kit::traffic_lights(window, close)),
        )
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
                let library = self.state.library.clone();
                d.child(
                    div()
                        .id("library-status")
                        .max_w(px(480.))
                        .truncate()
                        .text_color(palette::text_muted())
                        // While scanning the status is the full path of the
                        // file under the cursor: smaller text.
                        .when(scanning, |d| d.text_xs())
                        // The count's hover card: the catalog's totals, the
                        // status strip's tooltip. Only at idle, where the
                        // text is the count the card expands on.
                        .when(idle, |d| {
                            d.tooltip(move |_window, cx| {
                                rox_panels::status::library_tooltip(&library, cx)
                            })
                        })
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
            // Between the scan badge and the scan controls: it's the same
            // kind of thing, work the library is doing that you didn't have
            // to sit and watch. Always there, since the window behind it is
            // also where those jobs are started from.
            .child(crate::tasks_window::control(cx))
            .when(can_rescan && idle, |d| {
                d.child(panel::icon_control_sized(
                    icons::REFRESH_CW,
                    px(12.),
                    palette::text_muted(),
                    "Rescan the library folders",
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
                    "Stop the scan",
                    |this: &mut Workspace, cx| {
                        this.state
                            .library
                            .update(cx, |library, cx| library.abort_scan(cx));
                    },
                    cx,
                ))
            })
    }

    /// A paint-time capture of a menu surface's bounds into
    /// [`Workspace::menu_surfaces`], with the viewport width alongside:
    /// what the next frame's flyout side decisions read.
    fn menu_surface_capture(&self, level: usize, cx: &mut Context<Self>) -> impl IntoElement {
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

    /// The side decision for a flyout off the surface at `level`, from the
    /// bounds captured at the last paint.
    fn flyout_left(&self, level: usize) -> bool {
        flyout_leftward(&self.menu_surfaces, level, self.menu_viewport_w)
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
            .child(self.menu_surface_capture(0, cx))
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
                                this.open_subgroup = None;
                                cx.notify();
                            }
                        }))
                        .into_any_element(),
                    MenuEntry::Section(label) => menu_section(label).into_any_element(),
                    // A gated-off section draws nothing rather than an
                    // empty group row.
                    MenuEntry::Panels(section) if !section_shows(section) => {
                        div().into_any_element()
                    }
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
                                            this.open_subgroup = None;
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
                    MenuEntry::PresetsSubmenu {
                        label,
                        icon,
                        target,
                    } => self
                        .presets_submenu_row(i, label, icon, *target, cx)
                        .into_any_element(),
                    MenuEntry::PanelWindowsSubmenu { label, icon } => self
                        .panel_windows_submenu_row(i, label, icon, cx)
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
            MenuAction::ToggleDesignMode => settings::design_mode(),
            MenuAction::ToggleDecorations => settings::os_decorations(),
            MenuAction::ToggleQuitToTray => settings::quit_to_tray(),
            MenuAction::ToggleArtTheming => palette::art_theming(),
            MenuAction::TogglePostShader => crate::workspace::post_shader_on(),
            _ => false,
        };
        let is_playing = self.state.player.read(cx).is_playing();
        let (label, icon) = menu_item_display(item, is_playing);
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
                    this.open_subgroup = None;
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
                    .path(icon)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child(label)
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
            // Panels with knobs the signal pool can drive say so here, so
            // the list itself answers which ones the pool reaches.
            .when(signal_marked(action), |d| {
                d.child(div().flex_1().min_w(px(24.))).child(
                    svg()
                        .path(icons::AUDIO_WAVEFORM)
                        .size_3()
                        .text_color(palette::text_faint()),
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
                    this.open_subgroup = None;
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
                    flyout_side(div().absolute(), self.flyout_left(0))
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
    /// items are the saved presets, read when it opens, each
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
                    this.open_subgroup = None;
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
                    .child(rox_i18n::t!(label)),
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
                let presets = rox_core::settings::layouts::all(&Settings::load());
                let mut flyout = flyout_side(div().absolute(), self.flyout_left(0))
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
                                .child(rox_i18n::t!("menu-no-layouts")),
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
                    this.open_subgroup = None;
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
            .child(rox_i18n::t!("menu-new-ellipsis"))
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
                    this.open_subgroup = None;
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

    /// The panel-presets flyout: the saved panels, read when it opens, each
    /// doing the flyout's `target` - built into this window, or opened in one
    /// of its own.
    fn presets_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: PanelTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        submenu_shell(index, label, icon, open, cx).when(open, |d| {
            // Read the presets only once the flyout opens, not on every
            // parent-menu paint.
            let presets = panel_presets::saved();
            let flyout = flyout_box(self.flyout_left(0));
            d.child(if presets.is_empty() {
                flyout.child(flyout_note(rox_i18n::t!("menu-no-presets")))
            } else {
                flyout.children(
                    presets
                        .into_iter()
                        .map(|preset| self.preset_item(preset, target, cx)),
                )
            })
        })
    }

    /// The Window menu's panel picker: one flyout of groups - the saved
    /// presets, then the catalog's own - each flying out again into its
    /// panels. Every pick opens that panel in a window of its own, which is
    /// why this is a flyout of its own rather than a target on the Panels
    /// menu.
    fn panel_windows_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_submenu == Some(index);
        submenu_shell(index, label, icon, open, cx).when(open, |d| {
            let presets = panel_presets::saved();
            // This flyout hosts the group flyouts, so it captures its own
            // bounds for their side decision.
            let mut flyout =
                flyout_box(self.flyout_left(0)).child(self.menu_surface_capture(1, cx));
            // Group 0 is the presets when there are any, so the catalog's
            // groups start one along and the two levels never share an index.
            if !presets.is_empty() {
                let rows = presets
                    .into_iter()
                    .map(|preset| self.preset_item(preset, PanelTarget::NewWindow, cx))
                    .collect();
                flyout = flyout.child(self.panel_window_group(
                    0,
                    panel_presets::GROUP_LABEL,
                    panel_presets::GROUP_ICON,
                    rows,
                    cx,
                ));
            }
            for (i, section) in catalog::sections().enumerate() {
                let rows = section
                    .panels
                    .iter()
                    .map(|def| self.panel_window_item(def, cx))
                    .collect::<Vec<_>>();
                flyout = match section.group {
                    // A bare section is a run of plain rows in place, the
                    // same as everywhere else the catalog is drawn.
                    None => flyout.children(rows),
                    Some((label, icon)) => {
                        flyout.child(self.panel_window_group(i + 1, label, icon, rows, cx))
                    }
                };
            }
            d.child(flyout)
        })
    }

    /// One group inside the panel picker: its own flyout, a level deeper than
    /// the menus' usual one. `index` is the group's slot in that picker, kept
    /// apart from the entry indices the level above uses.
    fn panel_window_group(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        rows: Vec<Div>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_subgroup == Some(index);
        div()
            .id(("panel-window-group", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open, |d| d.bg(palette::bg_control_hover_opaque()))
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_subgroup != Some(index) {
                    this.open_subgroup = Some(index);
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
                d.child(flyout_box(self.flyout_left(1)).children(rows))
            })
    }

    /// A catalog row in the panel picker: closes the menu, then opens that
    /// panel with its stock config in a window of its own.
    fn panel_window_item(&self, def: &'static PanelDef, cx: &mut Context<Self>) -> Div {
        menu_row(cx.listener(move |this, _, window, cx| {
            this.close_menus(cx);
            this.open_panel_window(def, window, cx);
        }))
        .child(
            svg()
                .path(def.icon)
                .size_3p5()
                .text_color(palette::text_muted()),
        )
        .child(rox_i18n::t!(def.label))
    }

    /// A preset row in a presets flyout: closes the menu, then does the
    /// flyout's `target` with the named preset.
    fn preset_item(
        &self,
        preset: rox_core::settings::PanelPreset,
        target: PanelTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let icon = panel_presets::icon_for(&preset);
        let label = SharedString::from(preset.name.clone());
        let name = preset.name;
        menu_row(cx.listener(move |this, _, window, cx| {
            this.close_menus(cx);
            this.run_panel_preset(name.clone(), target, window, cx);
        }))
        .child(
            svg()
                .path(icon)
                .size_3p5()
                .text_color(palette::text_muted()),
        )
        .child(label)
    }

    /// Close whatever the menubar has open, down to the nested flyouts. What
    /// every row that runs something does first.
    fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.open_submenu = None;
        self.open_subgroup = None;
        cx.notify();
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
                    this.open_subgroup = None;
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
                    .child(rox_i18n::t!(label)),
            )
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                // Read the workspaces only once the flyout opens, not on every
                // parent-menu paint, and only far enough to name them: the
                // bundles stay on disk until one is applied. The Save flyout
                // can't overwrite shipped bundles, so it drops them, matching
                // the settings window where shipped rows carry no Overwrite.
                let mut entries = crate::workspaces::all();
                if target == WorkspaceTarget::Overwrite {
                    entries.retain(|entry| !entry.builtin);
                }
                let mut flyout = flyout_side(div().absolute(), self.flyout_left(0))
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
                                .child(rox_i18n::t!("menu-no-workspaces")),
                        );
                    }
                } else {
                    flyout = flyout.children(entries.into_iter().map(|entry| {
                        self.workspace_item(entry.name, entry.title, entry.builtin, target, cx)
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
                    this.open_subgroup = None;
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
            .child(rox_i18n::t!("menu-new-ellipsis"))
    }

    /// A workspace row in a workspaces flyout: closes the menu, then stages
    /// the flyout's confirm with the named bundle. A shipped bundle trails a
    /// muted tag to tell it from the user's own.
    fn workspace_item(
        &self,
        name: String,
        title: gpui::SharedString,
        builtin: bool,
        target: WorkspaceTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = title;
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
                    this.open_subgroup = None;
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
                        .child(rox_i18n::t!("menu-workspace-builtin-tag")),
                )
            })
    }
}

/// Flip design mode, from wherever it was asked for: the Window menu's row,
/// the Appearance page's toggle, or the row at the top of every panel menu.
/// The live flag repaints every window, the file keeps it across launches,
/// and the native bar redraws its label.
pub(crate) fn toggle_design_mode(cx: &mut App) {
    let on = !settings::design_mode();
    settings::set_design_mode(on, cx);
    Settings::update(move |s| s.design_mode = on);
    native_menu::rebuild(cx);
}

/// A flyout row's own chrome: the icon, the label, the chevron, and the hover
/// that opens it at `index`. The flyout itself is the caller's, chained onto
/// what comes back - it's the part that differs between a static group and a
/// list read at open time.
fn submenu_shell(
    index: usize,
    label: &'static str,
    icon: &'static str,
    open: bool,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
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
                this.open_subgroup = None;
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
                .child(rox_i18n::t!(label)),
        )
        .child(
            svg()
                .path(icons::CHEVRON_RIGHT)
                .size_3()
                .text_color(palette::text_muted()),
        )
}

/// The panel a flyout's items sit in, beside the row that opened it on the
/// side [`flyout_leftward`] picked. The top offset backs out the parent's
/// padding and the dropdown border so the first item lines up with that row.
fn flyout_box(leftward: bool) -> Div {
    flyout_side(div().absolute(), leftward)
        .top(px(-5.))
        .min_w(px(180.))
        .flex()
        .flex_col()
        .py(tokens::SPACE_XS)
        .bg(palette::bg_menu_opaque())
        .border_1()
        .border_color(palette::border_light())
        .shadow_md()
        .occlude()
}

/// What a flyout says instead of its items when it has none.
fn flyout_note(text: impl Into<SharedString>) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .text_color(palette::text_muted())
        .child(text.into())
}

/// The Alt tap tracker behind the menubar pin. Alt held floats a hidden bar
/// over the dock, and two quick taps of it pin the bar up so it stays with
/// nothing held. Holding the key is what makes the plain reveal awkward to
/// click: Alt+drag is the compositor's window move, and on macOS
/// Option-click on the zoom light means zoom, so fullscreen was unreachable
/// while the bar only existed under a held Option.
///
/// Only a clean tap counts: Alt alone, released quickly, with nothing under
/// it. A chord, a drag, or a long hold cancels the run. Split off the
/// workspace so the timing rules can be exercised without a window.
#[derive(Default)]
pub(crate) struct AltTap {
    /// When the current Alt press started, or None once it's been ruled out
    /// as a tap.
    held_since: Option<Instant>,
    /// When the last clean tap released, the window a second tap has to
    /// land in to make the pair.
    tapped_at: Option<Instant>,
}

impl AltTap {
    /// Feed the tracker a modifiers change. Reports true on the release that
    /// completes a double-tap, which is the caller's cue to toggle the pin.
    fn note(&mut self, modifiers: Modifiers, now: Instant, pointer_down: bool) -> bool {
        if modifiers.alt {
            let alone = !modifiers.control
                && !modifiers.shift
                && !modifiers.platform
                && !modifiers.function;
            if !alone || pointer_down {
                self.cancel();
            } else if self.held_since.is_none() {
                self.held_since = Some(now);
            }
            return false;
        }
        let Some(held) = self.held_since.take() else {
            self.tapped_at = None;
            return false;
        };
        if now.duration_since(held) > ALT_TAP_MAX {
            self.tapped_at = None;
            return false;
        }
        match self.tapped_at.take() {
            Some(first) if now.duration_since(first) <= ALT_DOUBLE_TAP => true,
            _ => {
                self.tapped_at = Some(now);
                false
            }
        }
    }

    /// A key or a button landed under the held Alt, so it's a chord or a
    /// drag; the press and the pair it might have completed are both off.
    fn cancel(&mut self) {
        self.held_since = None;
        self.tapped_at = None;
    }
}

/// A clickable flyout row: the padding, hover, and icon-then-label layout
/// every one of them shares. The caller chains the icon and label on.
fn menu_row(on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .cursor_pointer()
        .hover(|d| d.bg(palette::bg_control_hover_opaque()))
        .on_mouse_down(MouseButton::Left, on_click)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
}

#[cfg(test)]
mod alt_tap_tests {
    use super::*;

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Default::default()
        }
    }

    fn none() -> Modifiers {
        Modifiers::default()
    }

    /// Press and release Alt at the given offsets from a fixed start, so the
    /// timings are the test's to pick rather than the clock's.
    fn tap(tracker: &mut AltTap, base: Instant, down: u64, up: u64) -> bool {
        tracker.note(alt(), base + Duration::from_millis(down), false);
        tracker.note(none(), base + Duration::from_millis(up), false)
    }

    #[test]
    fn two_quick_taps_fire() {
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 50));
        assert!(tap(&mut tracker, base, 200, 250));
    }

    #[test]
    fn a_third_tap_starts_a_fresh_pair() {
        // The pair is consumed when it fires, so the tap after it is a first
        // tap again and only the fourth toggles back.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 50));
        assert!(tap(&mut tracker, base, 100, 150));
        assert!(!tap(&mut tracker, base, 200, 250));
        assert!(tap(&mut tracker, base, 300, 350));
    }

    #[test]
    fn a_slow_second_tap_misses_the_window() {
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 50));
        assert!(!tap(&mut tracker, base, 900, 950));
    }

    #[test]
    fn a_held_alt_is_not_a_tap() {
        // The plain reveal: Alt down, the bar floats, Alt up a second later.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 1000));
        assert!(!tap(&mut tracker, base, 1100, 1150));
    }

    #[test]
    fn a_chord_is_not_a_tap() {
        // Alt+Shift, then a clean tap: the chord can't be half of a pair.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        let chord = Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        };
        tracker.note(chord, base, false);
        assert!(!tracker.note(none(), base + Duration::from_millis(50), false));
        assert!(!tap(&mut tracker, base, 100, 150));
    }

    #[test]
    fn a_key_under_alt_cancels_the_run() {
        // What the workspace's captured key handler does: alt-f4 and friends
        // are chords, so the release that follows isn't a tap.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 50));
        tracker.note(alt(), base + Duration::from_millis(100), false);
        tracker.cancel();
        assert!(!tracker.note(none(), base + Duration::from_millis(150), false));
    }

    #[test]
    fn an_alt_drag_is_not_a_tap() {
        // Alt pressed with a button already down is the compositor's window
        // move, not a tap, however short it is.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert!(!tap(&mut tracker, base, 0, 50));
        tracker.note(alt(), base + Duration::from_millis(100), true);
        assert!(!tracker.note(none(), base + Duration::from_millis(150), false));
    }
}
