//! The menubar: the dropdown menus, their layout and workspace flyout
//! submenus, and menu action dispatch. Split out of the workspace shell it
//! renders into; it touches the same private state, so these are
//! `impl Workspace` methods in a child module.

use super::*;

use gpui::MouseDownEvent;

/// Where the keyboard cursor sits inside an open dropdown: the entry it's
/// on, plus the row within it for the one entry kind that draws a run of
/// them (a catalog section with no group of its own). A position rather
/// than a flat row count, so the renderer can compare against it while
/// walking entries, without counting what came before.
pub(crate) type NavSlot = (usize, Option<usize>);

/// What Enter on the row under the cursor does. The flyouts' rows come off
/// disk and out of the catalog rather than the static table, so there's no
/// single action type that covers them.
pub(crate) enum NavRun {
    Action(MenuAction),
    Layout(String, LayoutTarget),
    Workspace(String, WorkspaceTarget),
    Preset(String, PanelTarget),
    PanelWindow(&'static PanelDef),
    /// The "New..." rows, which open a save dialog rather than run.
    SaveLayout,
    SaveWorkspace,
}

/// One row the cursor can land on: something to run, or a surface to step
/// into. `Open` carries the index that level's `open_*` field wants, which
/// isn't the row's own position once headings and hidden sections have been
/// skipped.
pub(crate) enum NavRow {
    Run(NavRun),
    Open(usize),
}

/// Where the Application menu's three project links go. The issue form is
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

    /// The modifiers changed, which is where the pin and the access letters
    /// come from. A docked bar arms on a single clean tap of Alt and drops
    /// again on the next one. A hidden bar has nothing to arm until it's up,
    /// so there the pair does both at once: pin the bar and arm it.
    pub(crate) fn note_modifiers(&mut self, modifiers: Modifiers, cx: &mut Context<Self>) {
        let tap = self
            .alt_tap
            .note(modifiers, Instant::now(), self.pointer_down);
        if tap == AltTapKind::None {
            return;
        }
        if !settings::hide_menubar() {
            self.set_menu_keys(!self.menubar_keys, cx);
            return;
        }
        if tap == AltTapKind::DoubleTap {
            self.menubar_pinned = !self.menubar_pinned;
            self.menubar_touched = false;
            self.set_menu_keys(self.menubar_pinned, cx);
            cx.notify();
        }
    }

    /// Arm or drop the menubar's keyboard mode: the access letters, the
    /// cursor, and the `MenuNav` context that hands space and the arrows
    /// back from the playback bindings. Dropping it closes whatever the
    /// keyboard opened. Never arms on macOS, where the menus live in the
    /// system bar and this row has no buttons to walk.
    fn set_menu_keys(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && !cfg!(target_os = "macos");
        if self.menubar_keys == on {
            return;
        }
        self.menubar_keys = on;
        if on {
            self.menu_top = self.open_menu.unwrap_or(0);
        } else {
            self.close_menus(cx);
        }
        cx.notify();
    }

    /// Drop the keyboard mode from outside the Alt path: a click that takes
    /// the bar back to the mouse, or an Escape at the end of its ladder.
    /// Reports whether there was one to drop.
    pub(crate) fn drop_menu_keys(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.menubar_keys {
            return false;
        }
        self.set_menu_keys(false, cx);
        true
    }

    /// A key or a button went down under the held Alt, so it's a chord or a
    /// drag rather than a tap, and the pair it might have completed is off
    /// too.
    pub(crate) fn cancel_alt_tap(&mut self) {
        self.alt_tap.cancel();
    }

    /// Drop a pinned menubar. Reports whether there was one, so escape can
    /// stop at the bar instead of falling through to what it backs out of
    /// next.
    ///
    /// The keyboard mode goes with it. On a hidden bar the pin is what holds
    /// the row on screen, and an armed bar keeps it up on its own, so leaving
    /// the letters behind would strand the bar over the dock with nothing but
    /// Escape to clear it.
    pub(crate) fn unpin_menubar(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.menubar_pinned {
            return false;
        }
        self.menubar_pinned = false;
        self.menubar_touched = false;
        self.set_menu_keys(false, cx);
        cx.notify();
        true
    }

    /// A keystroke offered to the menubar while it's taking keys. Reports
    /// whether it was used, which is the caller's cue to stop the event: the
    /// bar has the keyboard until Escape gives it back, so a letter must not
    /// also land in whatever panel holds focus underneath.
    ///
    /// Bindings beat key listeners, so the bar's `MenuNav` context is what
    /// really hands space and the arrows over (see `keymap::PLAYBACK`); this
    /// only sees what nothing bound took first.
    pub(crate) fn menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.menubar_keys {
            return false;
        }
        let modifiers = event.keystroke.modifiers;
        // A real chord still belongs to whatever binds it. Shift doesn't
        // count: it's how you type a capital, and the letters match either
        // way.
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.menu_escape(cx),
            "up" => self.menu_step(-1, cx),
            "down" => self.menu_step(1, cx),
            "left" => self.menu_out(cx),
            "right" => self.menu_in(cx),
            "enter" | "space" => self.menu_enter(window, cx),
            key => {
                if let Some(index) = mnemonic_menu(key) {
                    self.open_top(index, cx);
                }
            }
        }
        // Everything else is swallowed rather than passed down. The bar was
        // armed on purpose and Escape is the way out of it, so a stray key
        // reaching the panel underneath would be the surprise.
        true
    }

    /// Escape's ladder: back out one surface at a time, then off the bar. A
    /// hidden bar was pinned up by the same double-tap that armed it, so the
    /// last rung drops both.
    fn menu_escape(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.menu_group_slot = None;
            cx.notify();
        } else if self.open_submenu.is_some() {
            self.open_flyout(None);
            cx.notify();
        } else if self.open_menu.is_some() {
            self.close_menus(cx);
        } else {
            self.set_menu_keys(false, cx);
            self.unpin_menubar(cx);
        }
    }

    /// Up and down: move the cursor within the deepest open surface,
    /// wrapping at both ends. With nothing dropped down they open the
    /// cursor's menu instead, down at its first row and up at its last.
    fn menu_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.open_menu.is_none() {
            self.open_top(self.menu_top, cx);
            if delta < 0 {
                self.menu_slot = self.menu_rows().last().map(|(slot, _)| *slot);
                cx.notify();
            }
            return;
        }
        if self.open_subgroup.is_some() {
            self.menu_group_slot = step_index(self.menu_group_slot, delta, self.group_rows().len());
        } else if self.open_submenu.is_some() {
            self.menu_sub_slot = step_index(self.menu_sub_slot, delta, self.flyout_rows().len());
        } else {
            let rows = self.menu_rows();
            let at = self
                .menu_slot
                .and_then(|slot| rows.iter().position(|(row, _)| *row == slot));
            self.menu_slot = step_index(at, delta, rows.len()).map(|i| rows[i].0);
        }
        cx.notify();
    }

    /// Left: back out of a flyout, or step to the menu before this one.
    fn menu_out(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.menu_group_slot = None;
        } else if self.open_submenu.is_some() {
            self.open_flyout(None);
        } else if self.open_menu.is_some() {
            self.open_top(self.step_top(-1), cx);
            return;
        } else {
            self.menu_top = self.step_top(-1);
        }
        cx.notify();
    }

    /// Right: step into the flyout under the cursor, or on a row that has
    /// none, on to the next menu. Inside the panel picker's group flyout
    /// there's nothing deeper to step into, so it holds still.
    fn menu_in(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            return;
        }
        if self.open_submenu.is_some() {
            if let Some(NavRow::Open(group)) = self.row_at(self.flyout_rows(), self.menu_sub_slot) {
                self.open_subgroup = Some(group);
                self.menu_group_slot = (!self.group_rows().is_empty()).then_some(0);
                cx.notify();
            }
            return;
        }
        if self.open_menu.is_some() {
            if let Some(NavRow::Open(entry)) = self.current_row() {
                self.open_flyout(Some(entry));
                self.menu_sub_slot = (!self.flyout_rows().is_empty()).then_some(0);
                cx.notify();
            } else {
                self.open_top(self.step_top(1), cx);
            }
            return;
        }
        self.menu_top = self.step_top(1);
        cx.notify();
    }

    /// Enter: run the row under the cursor, or open it when it's a flyout.
    /// With nothing dropped down it drops the cursor's menu, same as down.
    fn menu_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_menu.is_none() {
            self.open_top(self.menu_top, cx);
            return;
        }
        let row = if self.open_subgroup.is_some() {
            self.row_at(self.group_rows(), self.menu_group_slot)
        } else if self.open_submenu.is_some() {
            self.row_at(self.flyout_rows(), self.menu_sub_slot)
        } else {
            self.current_row()
        };
        match row {
            Some(NavRow::Run(run)) => self.nav_run(run, window, cx),
            Some(NavRow::Open(_)) => self.menu_in(cx),
            None => {}
        }
    }

    /// Drop the menu at `index` under the keyboard, cursor on its first row.
    fn open_top(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close_menus(cx);
        self.menu_top = index;
        self.open_menu = Some(index);
        self.menu_slot = self.menu_rows().first().map(|(slot, _)| *slot);
        cx.notify();
    }

    /// The menu one step along the bar from the cursor's, wrapping.
    fn step_top(&self, delta: isize) -> usize {
        let len = MENUS.len() as isize;
        (self.menu_top as isize + delta).rem_euclid(len) as usize
    }

    /// Run what the keyboard picked, then get off the bar: the mouse rows do
    /// the same, and leaving the letters up over a window that just opened
    /// would keep eating its keys.
    fn nav_run(&mut self, run: NavRun, window: &mut Window, cx: &mut Context<Self>) {
        self.set_menu_keys(false, cx);
        self.run_nav(run, window, cx);
    }

    /// Do what a picked row says, whichever menu picked it. The menu panel
    /// comes straight here: it has no access letters to put away first, so
    /// there's nothing for it in the step above.
    pub(crate) fn run_nav(&mut self, run: NavRun, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            NavRun::Action(action) => self.run(action, window, cx),
            NavRun::Layout(name, target) => self.run_layout(name, target, cx),
            NavRun::Workspace(name, target) => self.run_workspace(name, target, cx),
            NavRun::Preset(name, target) => self.run_panel_preset(name, target, window, cx),
            NavRun::PanelWindow(def) => self.open_panel_window(def, window, cx),
            NavRun::SaveLayout => self.open_save_dialog(window, cx),
            NavRun::SaveWorkspace => self.open_save_workspace_dialog(window, cx),
        }
    }

    /// The row a cursor index picks out of a flyout's list.
    fn row_at(&self, rows: Vec<NavRow>, at: Option<usize>) -> Option<NavRow> {
        nav_row_at(rows, at)
    }

    /// The dropdown row the cursor sits on.
    fn current_row(&self) -> Option<NavRow> {
        let slot = self.menu_slot?;
        self.menu_rows()
            .into_iter()
            .find(|(row, _)| *row == slot)
            .map(|(_, row)| row)
    }

    /// The open dropdown's rows in draw order.
    fn menu_rows(&self) -> Vec<(NavSlot, NavRow)> {
        self.open_menu.map(menu_entry_rows).unwrap_or_default()
    }

    /// The open flyout's rows in draw order.
    fn flyout_rows(&self) -> Vec<NavRow> {
        match (self.open_menu, self.open_submenu) {
            (Some(menu), Some(entry)) => submenu_rows(menu, entry),
            _ => Vec::new(),
        }
    }

    /// The open group flyout's rows in draw order.
    fn group_rows(&self) -> Vec<NavRow> {
        self.open_subgroup.map(subgroup_rows).unwrap_or_default()
    }
}

/// The row a cursor index picks out of a flyout's list.
pub(crate) fn nav_row_at(rows: Vec<NavRow>, at: Option<usize>) -> Option<NavRow> {
    rows.into_iter().nth(at?)
}

/// The dropdown rows of the menu at `menu` in draw order, each with the slot
/// the cursor uses for it. Headings and gated-off sections contribute none,
/// which is what makes stepping skip past them.
///
/// Off the table and an index rather than off menubar state, because the menu
/// panel draws the same `MENUS` a level deeper and walks it with the same
/// three functions. One reading of the table, so the keyboard can't come to a
/// different answer in the two places.
pub(crate) fn menu_entry_rows(menu: usize) -> Vec<(NavSlot, NavRow)> {
    let Some(menu) = MENUS.get(menu) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for (i, entry) in menu.entries.iter().enumerate() {
        match entry {
            MenuEntry::Item(item) => {
                rows.push(((i, None), NavRow::Run(NavRun::Action(item.action))))
            }
            MenuEntry::Section(_) => {}
            MenuEntry::Panels(section) if !section_shows(section) => {}
            // A bare section draws a run of rows in place, so its slots
            // carry the row within the entry as well as the entry.
            MenuEntry::Panels(section) if section.group.is_none() => {
                rows.extend(section.panels.iter().enumerate().map(|(j, def)| {
                    (
                        (i, Some(j)),
                        NavRow::Run(NavRun::Action(MenuAction::OpenPanel(def))),
                    )
                }))
            }
            _ => rows.push(((i, None), NavRow::Open(i))),
        }
    }
    rows
}

/// The rows of the flyout hanging off entry `entry` of the menu at `menu`, in
/// draw order, read from the same lists the flyout drew from so the two walk
/// in step. The note an empty flyout shows isn't a row, and never sits beside
/// one.
pub(crate) fn submenu_rows(menu: usize, entry: usize) -> Vec<NavRow> {
    let Some(entry) = MENUS.get(menu).and_then(|menu| menu.entries.get(entry)) else {
        return Vec::new();
    };
    match entry {
        MenuEntry::Panels(section) => section
            .panels
            .iter()
            .map(|def| NavRow::Run(NavRun::Action(MenuAction::OpenPanel(def))))
            .collect(),
        MenuEntry::LayoutsSubmenu {
            target, with_new, ..
        } => {
            let mut rows = Vec::new();
            if *with_new {
                rows.push(NavRow::Run(NavRun::SaveLayout));
            }
            rows.extend(
                rox_core::settings::layouts::all(&Settings::load())
                    .into_iter()
                    .map(|preset| NavRow::Run(NavRun::Layout(preset.name, *target))),
            );
            rows
        }
        MenuEntry::WorkspacesSubmenu {
            target, with_new, ..
        } => {
            let mut rows = Vec::new();
            if *with_new {
                rows.push(NavRow::Run(NavRun::SaveWorkspace));
            }
            let mut entries = crate::workspaces::all();
            if *target == WorkspaceTarget::Overwrite {
                entries.retain(|entry| !entry.builtin);
            }
            rows.extend(
                entries
                    .into_iter()
                    .map(|entry| NavRow::Run(NavRun::Workspace(entry.name, *target))),
            );
            rows
        }
        MenuEntry::PresetsSubmenu { target, .. } => panel_presets::saved()
            .into_iter()
            .map(|preset| NavRow::Run(NavRun::Preset(preset.name, *target)))
            .collect(),
        MenuEntry::PanelWindowsSubmenu { .. } => {
            let mut rows = Vec::new();
            // Group 0 is the presets when there are any, the same
            // numbering the picker draws with.
            if !panel_presets::saved().is_empty() {
                rows.push(NavRow::Open(0));
            }
            for (i, section) in catalog::sections().enumerate() {
                match section.group {
                    None => rows.extend(
                        section
                            .panels
                            .iter()
                            .map(|def| NavRow::Run(NavRun::PanelWindow(def))),
                    ),
                    Some(_) => rows.push(NavRow::Open(i + 1)),
                }
            }
            rows
        }
        MenuEntry::Item(_) | MenuEntry::Section(_) => Vec::new(),
    }
}

/// The rows of the picker group at `group`, the one surface a level deeper
/// than the rest: the panel picker's presets group, or one catalog group.
pub(crate) fn subgroup_rows(group: usize) -> Vec<NavRow> {
    let presets = panel_presets::saved();
    if group == 0 {
        return presets
            .into_iter()
            .map(|preset| NavRow::Run(NavRun::Preset(preset.name, PanelTarget::NewWindow)))
            .collect();
    }
    catalog::sections()
        .nth(group - 1)
        .map(|section| {
            section
                .panels
                .iter()
                .map(|def| NavRow::Run(NavRun::PanelWindow(def)))
                .collect()
        })
        .unwrap_or_default()
}

impl Workspace {
    /// Whether the keyboard cursor sits on the dropdown row at `slot`.
    fn nav_on(&self, entry: usize, row: Option<usize>) -> bool {
        self.menu_slot == Some((entry, row))
    }

    /// Whether the cursor sits on the open flyout's row at `row`.
    fn nav_sub(&self, row: usize) -> bool {
        self.menu_sub_slot == Some(row)
    }

    /// Whether the cursor sits on the flyout row that opens `group`, the
    /// picker's group headers. They're addressed by their place in the
    /// flyout, not by the group number they carry.
    fn nav_on_group(&self, group: usize) -> bool {
        matches!(
            self.row_at(self.flyout_rows(), self.menu_sub_slot),
            Some(NavRow::Open(open)) if open == group
        )
    }

    /// Whether the cursor sits on `row` of the picker group `group`, the one
    /// surface a level deeper than the rest. Group rows are built whether or
    /// not their group is open, so the group has to match too.
    fn nav_group(&self, group: usize, row: usize) -> bool {
        self.open_subgroup == Some(group) && self.menu_group_slot == Some(row)
    }

    /// The pointer crossing the pinned bar's edge. Entering arms the pin,
    /// leaving drops it, so the bar clears itself once it's been used. A
    /// leave with a dropdown open is the pointer moving into the dropdown,
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
        letter: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.open_menu == Some(index);
        // The keyboard cursor lights the button the same way an open menu
        // does, so walking the bar with the arrows reads as one cursor
        // whether or not anything is dropped down.
        let cursor = self.menubar_keys && self.menu_top == index;
        div()
            .relative()
            .h_full()
            .px(tokens::SPACE_MD)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(open || cursor, |d| d.bg(palette::bg_control_active()))
            .hover(|d| d.bg(palette::bg_menu_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let reopen = this.open_menu != Some(index);
                    this.close_menus(cx);
                    if reopen {
                        this.open_menu = Some(index);
                        this.menu_top = index;
                    }
                }),
            )
            // Clicking anywhere outside this button closes its menu; a click
            // that lands on a dropdown item still runs the item's handler.
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menus(cx)))
            })
            .child(menu_label(menu.label, letter))
            .when(open, |d| d.child(deferred(self.dropdown(menu, cx))))
    }
    /// The menubar row: the mini toggle, the menus, and the status side.
    /// One builder so the docked row and the alt-revealed overlay stay
    /// the same bar.
    pub(crate) fn menubar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        // On macOS the menus are in the system bar, so this row keeps only
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
                // The letters are only worked out while they're on show, so
                // the common frame doesn't pay for five locale lookups and a
                // dedup pass.
                let letters = self.menubar_keys.then(mnemonics).unwrap_or_default();
                d.children(MENUS.iter().enumerate().map(|(i, menu)| {
                    let letter = letters.get(i).cloned().flatten().map(|(range, _)| range);
                    self.menu_button(i, menu, letter, cx)
                }))
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
            .when_some(crate::startup::updates::available(), |d, version| {
                d.child(self.update_chip(version, cx))
            })
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

    /// The "Update Available" chip beside the catalog status, shown while a
    /// newer release sits in the cache. The chip opens the About window,
    /// which offers the download where the install can replace itself and
    /// the release page everywhere else; the x dismisses it for this
    /// release, so it only returns with the next one.
    fn update_chip(&self, version: String, cx: &mut Context<Self>) -> impl IntoElement {
        let dismiss_version = version.clone();
        let chip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .py(px(2.))
            .rounded_full()
            .bg(palette::accent())
            .text_xs()
            .text_color(palette::text_on_accent())
            .cursor_pointer()
            .hover(|d| d.bg(palette::accent_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    crate::startup::about_window::open(this.state.clone(), cx);
                }),
            )
            .child(rox_i18n::t!("menu-update-available"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                        cx.stop_propagation();
                        crate::startup::updates::dismiss(dismiss_version.clone());
                        cx.refresh_windows();
                    })
                    // The svg takes its color from its own style, the chip's
                    // text color doesn't reach it, so set it here.
                    .child(
                        svg()
                            .path(icons::CLOSE)
                            .size(px(10.))
                            .flex_none()
                            .text_color(palette::text_on_accent()),
                    ),
            );
        panel::Tip::keyed(
            "update-chip",
            rox_i18n::t!("about-version-available", version = version),
        )
        .apply(chip)
    }

    /// A paint-time capture of a menu surface's bounds into
    /// [`Workspace::menu_surfaces`], with the viewport width alongside. The
    /// next frame's flyout side decisions read both.
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
                                this.open_flyout(None);
                                cx.notify();
                            }
                        }))
                        .when(self.nav_on(i, None), nav_lit)
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
                                            this.open_flyout(None);
                                            cx.notify();
                                        }
                                    }))
                                    .when(self.nav_on(i, Some(j)), nav_lit)
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
        // The static menu table can't hold state, so the toggle row reads
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
                    this.close_menus(cx);
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
            // Panels with knobs the signal pool can drive are marked here, so
            // the list itself shows which ones the pool can reach.
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
            .when(open || self.nav_on(index, None), nav_lit)
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_flyout(Some(index));
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
                        .children(panels.iter().enumerate().map(|(row, def)| {
                            self.action_item(panel_menu_item(def), cx)
                                .when(self.nav_sub(row), nav_lit)
                        })),
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
            .when(open || self.nav_on(index, None), nav_lit)
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_flyout(Some(index));
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
                    flyout = flyout.child(self.save_new_item(cx).when(self.nav_sub(0), nav_lit));
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
                    flyout =
                        flyout.children(presets.into_iter().enumerate().map(|(row, preset)| {
                            self.layout_item(preset.name, target, cx)
                                .when(self.nav_sub(row + usize::from(with_new)), nav_lit)
                        }));
                }
                d.child(flyout)
            })
    }

    /// The Save flyout's leading row: opens the save dialog for a fresh
    /// preset, closing the menu first like every other flyout row.
    fn save_new_item(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_menus(cx);
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
    /// flyout's thing with the named preset: open a window, overwrite it
    /// with the current arrangement, or apply it here behind a confirm.
    fn layout_item(&self, name: String, target: LayoutTarget, cx: &mut Context<Self>) -> Div {
        let label = SharedString::from(name.clone());
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.close_menus(cx);
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
    /// doing the flyout's `target`: built into this window, or opened in one
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
        let lit = open || self.nav_on(index, None);
        submenu_shell(index, label, icon, lit, cx).when(open, |d| {
            // Read the presets only once the flyout opens, not on every
            // parent-menu paint.
            let presets = panel_presets::saved();
            let flyout = flyout_box(self.flyout_left(0));
            d.child(if presets.is_empty() {
                flyout.child(flyout_note(rox_i18n::t!("menu-no-presets")))
            } else {
                flyout.children(presets.into_iter().enumerate().map(|(row, preset)| {
                    self.preset_item(preset, target, cx)
                        .when(self.nav_sub(row), nav_lit)
                }))
            })
        })
    }

    /// The Window menu's panel picker: one flyout of groups (the saved
    /// presets, then the catalog's own), each flying out again into its
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
        let lit = open || self.nav_on(index, None);
        submenu_shell(index, label, icon, lit, cx).when(open, |d| {
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
                    .enumerate()
                    .map(|(row, preset)| {
                        self.preset_item(preset, PanelTarget::NewWindow, cx)
                            .when(self.nav_group(0, row), nav_lit)
                    })
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
                    .enumerate()
                    .map(|(row, def)| {
                        self.panel_window_item(def, cx)
                            .when(self.nav_group(i + 1, row), nav_lit)
                    })
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
            .when(open || self.nav_on_group(index), nav_lit)
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_subgroup != Some(index) {
                    this.open_subgroup = Some(index);
                    this.menu_group_slot = None;
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
        self.menu_slot = None;
        self.open_submenu = None;
        self.menu_sub_slot = None;
        self.open_subgroup = None;
        self.menu_group_slot = None;
        cx.notify();
    }

    /// Move the flyout open off the dropdown entry at `index`, or shut it
    /// with None. The keyboard cursor goes with it: a hover that lands
    /// somewhere else must not leave a highlight behind in a list it no
    /// longer belongs to.
    fn open_flyout(&mut self, index: Option<usize>) {
        self.open_submenu = index;
        self.menu_sub_slot = None;
        self.open_subgroup = None;
        self.menu_group_slot = None;
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
            .when(open || self.nav_on(index, None), nav_lit)
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_submenu != Some(index) {
                    this.open_flyout(Some(index));
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
                // the settings window where shipped rows have no Overwrite.
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
                    flyout = flyout.child(
                        self.save_new_workspace_item(cx)
                            .when(self.nav_sub(0), nav_lit),
                    );
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
                    flyout =
                        flyout.children(entries.into_iter().enumerate().map(|(row, entry)| {
                            self.workspace_item(entry.name, entry.title, entry.builtin, target, cx)
                                .when(self.nav_sub(row + usize::from(with_new)), nav_lit)
                        }));
                }
                d.child(flyout)
            })
    }

    /// The Save Workspace flyout's leading row: opens the save dialog for a
    /// fresh bundle, closing the menu first like every other flyout row.
    fn save_new_workspace_item(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_menus(cx);
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
    ) -> Div {
        let label = title;
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.close_menus(cx);
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
/// what comes back: it's the part that differs between a static group and a
/// list read at open time.
fn submenu_shell(
    index: usize,
    label: &'static str,
    icon: &'static str,
    lit: bool,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<Div> {
    div()
        .id(("menu-entry", index))
        .relative()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .cursor_pointer()
        .when(lit, nav_lit)
        .hover(|d| d.bg(palette::bg_control_hover_opaque()))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered && this.open_submenu != Some(index) {
                this.open_flyout(Some(index));
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

/// The panel a flyout's items go in, beside the row that opened it on the
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

/// What a flyout shows instead of its items when it has none.
fn flyout_note(text: impl Into<SharedString>) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .text_color(palette::text_muted())
        .child(text.into())
}

/// The Alt tap tracker behind the menubar pin. Alt held floats a hidden bar
/// over the dock, and two quick taps of it pin the bar up so it stays with
/// nothing held. Holding the key makes the plain reveal awkward to
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

/// What an Alt release amounted to. A docked bar arms its access letters on
/// the single tap; a hidden one waits for the pair, since the first tap of
/// that pair has no bar to arm yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AltTapKind {
    /// Not a tap at all: a chord, a drag, or a hold.
    None,
    /// A clean tap with no partner in the window before it.
    Tap,
    /// The second of a pair, close enough behind the first to make one.
    DoubleTap,
}

impl AltTap {
    /// Feed the tracker a modifiers change. Reports what the release it
    /// completed amounted to, which is what the caller acts on.
    fn note(&mut self, modifiers: Modifiers, now: Instant, pointer_down: bool) -> AltTapKind {
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
            return AltTapKind::None;
        }
        let Some(held) = self.held_since.take() else {
            self.tapped_at = None;
            return AltTapKind::None;
        };
        if now.duration_since(held) > ALT_TAP_MAX {
            self.tapped_at = None;
            return AltTapKind::None;
        }
        match self.tapped_at.take() {
            Some(first) if now.duration_since(first) <= ALT_DOUBLE_TAP => AltTapKind::DoubleTap,
            _ => {
                self.tapped_at = Some(now);
                AltTapKind::Tap
            }
        }
    }

    /// A key or a button went down under the held Alt, so it's a chord or a
    /// drag; the press and the pair it might have completed are both off.
    fn cancel(&mut self) {
        self.held_since = None;
        self.tapped_at = None;
    }
}

/// The next cursor index in a list of `len` rows, wrapping at both ends.
/// With the cursor not on a row yet it lands on the first, or the last
/// stepping backwards.
pub(crate) fn step_index(at: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(at) = at else {
        return Some(if delta < 0 { len - 1 } else { 0 });
    };
    Some((at as isize + delta).rem_euclid(len as isize) as usize)
}

/// The access letter each top menu answers to, in `MENUS` order: where it
/// sits in the translated label, and the key that reaches it. First letter
/// where it's free, the next unclaimed one where it isn't, which is how
/// Window and Workspace end up on W and o. None once a label has nothing
/// left unclaimed; that menu keeps its keyboard route through the arrows.
///
/// Off the translated text rather than the message key, so the underline is
/// on the letter you actually press whatever the locale.
fn mnemonics() -> Vec<Option<(std::ops::Range<usize>, char)>> {
    let mut taken: Vec<char> = Vec::new();
    MENUS
        .iter()
        .map(|menu| {
            let label = rox_i18n::t_static(menu.label);
            let hit = label.char_indices().find_map(|(at, c)| {
                let key = c.to_lowercase().next()?;
                (c.is_alphanumeric() && !taken.contains(&key)).then(|| (at..at + c.len_utf8(), key))
            });
            if let Some((_, key)) = &hit {
                taken.push(*key);
            }
            hit
        })
        .collect()
}

/// The top menu a bare keypress opens, by its access letter. gpui hands
/// letters over as one-character keys, so anything longer is some other key
/// and matches nothing.
fn mnemonic_menu(key: &str) -> Option<usize> {
    let mut chars = key.chars();
    let pressed = chars.next().filter(|_| chars.next().is_none())?;
    mnemonics()
        .iter()
        .position(|hit| hit.as_ref().is_some_and(|(_, c)| *c == pressed))
}

/// A top menu's label, with its access letter underlined while the bar is
/// taking keys. A plain string the rest of the time, which is every frame
/// nobody has tapped Alt.
fn menu_label(label: &'static str, letter: Option<std::ops::Range<usize>>) -> AnyElement {
    let text = rox_i18n::t!(label);
    match letter {
        None => text.into_any_element(),
        Some(range) => gpui::StyledText::new(text)
            .with_highlights([(
                range,
                gpui::HighlightStyle {
                    underline: Some(gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: None,
                        wavy: false,
                    }),
                    ..Default::default()
                },
            )])
            .into_any_element(),
    }
}

/// The wash under the row the keyboard cursor is on. The same one hover
/// paints, so the pointer and the arrows share one cursor between them
/// rather than lighting two rows at once.
pub(crate) fn nav_lit<T: Styled>(d: T) -> T {
    d.bg(palette::bg_control_hover_opaque())
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
mod nav_tests {
    use super::*;

    /// The source locale, which is what a test run resolves against. A
    /// translation is free to run a label out of free letters; that menu
    /// keeps its keyboard route through the arrows.
    #[test]
    fn every_menu_gets_a_letter_of_its_own() {
        let letters = mnemonics();
        assert_eq!(letters.len(), MENUS.len());
        let mut seen = Vec::new();
        for (menu, hit) in MENUS.iter().zip(&letters) {
            let label = rox_i18n::t_static(menu.label);
            let (range, key) = hit
                .as_ref()
                .unwrap_or_else(|| panic!("{label} ran out of free letters"));
            assert_eq!(
                label[range.clone()].to_lowercase(),
                key.to_string(),
                "{label} underlines a letter it doesn't answer to"
            );
            assert!(
                !seen.contains(key),
                "{label} took a letter already spoken for"
            );
            seen.push(*key);
        }
    }

    #[test]
    fn a_letter_opens_the_menu_it_underlines() {
        for (i, hit) in mnemonics().iter().enumerate() {
            let (_, key) = hit.as_ref().expect("every menu has a letter");
            assert_eq!(mnemonic_menu(&key.to_string()), Some(i));
        }
    }

    #[test]
    fn a_named_key_is_not_a_letter() {
        // gpui hands named keys over spelled out, and "escape" starting with
        // an e must not read as the e menu.
        assert_eq!(mnemonic_menu("escape"), None);
        assert_eq!(mnemonic_menu("enter"), None);
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        assert_eq!(step_index(Some(2), 1, 3), Some(0));
        assert_eq!(step_index(Some(0), -1, 3), Some(2));
        // A cursor left behind by a list that has since shrunk still lands
        // somewhere in the new one rather than off the end.
        assert_eq!(step_index(Some(9), 1, 3), Some(1));
    }

    #[test]
    fn stepping_onto_a_fresh_list_starts_at_the_near_end() {
        assert_eq!(step_index(None, 1, 3), Some(0));
        assert_eq!(step_index(None, -1, 3), Some(2));
        assert_eq!(step_index(None, 1, 0), None);
    }
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
    fn tap(tracker: &mut AltTap, base: Instant, down: u64, up: u64) -> AltTapKind {
        tracker.note(alt(), base + Duration::from_millis(down), false);
        tracker.note(none(), base + Duration::from_millis(up), false)
    }

    #[test]
    fn two_quick_taps_fire() {
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 50), AltTapKind::Tap);
        assert_eq!(tap(&mut tracker, base, 200, 250), AltTapKind::DoubleTap);
    }

    #[test]
    fn a_third_tap_starts_a_fresh_pair() {
        // The pair is consumed when it fires, so the tap after it is a first
        // tap again and only the fourth toggles back.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 50), AltTapKind::Tap);
        assert_eq!(tap(&mut tracker, base, 100, 150), AltTapKind::DoubleTap);
        assert_eq!(tap(&mut tracker, base, 200, 250), AltTapKind::Tap);
        assert_eq!(tap(&mut tracker, base, 300, 350), AltTapKind::DoubleTap);
    }

    #[test]
    fn a_slow_second_tap_misses_the_window() {
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 50), AltTapKind::Tap);
        assert_eq!(tap(&mut tracker, base, 900, 950), AltTapKind::Tap);
    }

    #[test]
    fn a_held_alt_is_not_a_tap() {
        // The plain reveal: Alt down, the bar floats, Alt up a second later.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 1000), AltTapKind::None);
        assert_eq!(tap(&mut tracker, base, 1100, 1150), AltTapKind::Tap);
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
        assert_eq!(
            tracker.note(none(), base + Duration::from_millis(50), false),
            AltTapKind::None
        );
        assert_eq!(tap(&mut tracker, base, 100, 150), AltTapKind::Tap);
    }

    #[test]
    fn a_key_under_alt_cancels_the_run() {
        // What the workspace's captured key handler does: alt-f4 and friends
        // are chords, so the release that follows isn't a tap.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 50), AltTapKind::Tap);
        tracker.note(alt(), base + Duration::from_millis(100), false);
        tracker.cancel();
        assert_eq!(
            tracker.note(none(), base + Duration::from_millis(150), false),
            AltTapKind::None
        );
    }

    #[test]
    fn an_alt_drag_is_not_a_tap() {
        // Alt pressed with a button already down is the compositor's window
        // move, not a tap, however short it is.
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 50), AltTapKind::Tap);
        tracker.note(alt(), base + Duration::from_millis(100), true);
        assert_eq!(
            tracker.note(none(), base + Duration::from_millis(150), false),
            AltTapKind::None
        );
    }
}
