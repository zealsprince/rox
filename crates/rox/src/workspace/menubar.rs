//! The menubar: the dropdown menus, their flyout submenus, and menu action
//! dispatch. `impl Workspace` in a child module, since it touches the shell's
//! private state.

use super::*;

use gpui::{Corner, MouseDownEvent, Stateful, anchored, point};
use gpui_component::scroll::Scrollbar;
use rox_core::settings::MenubarButtons;

/// The entry the cursor is on, plus the row within it for a bare catalog
/// section, which draws a run of rows.
pub(crate) type NavSlot = (usize, Option<usize>);

pub(crate) enum NavRun {
    Action(MenuAction),
    Layout(String, LayoutTarget),
    Workspace(String, WorkspaceTarget),
    Preset(String, PanelTarget),
    PanelWindow(&'static PanelDef),
    SaveLayout,
    SaveWorkspace,
}

/// `Open` carries the index that level's `open_*` field wants, which isn't
/// the row's position once headings and hidden sections are skipped.
pub(crate) enum NavRow {
    Run(NavRun),
    Open(usize),
}

/// The chooser rather than a blank issue, so reports arrive on a template.
const ISSUES_URL: &str = "https://github.com/zealsprince/rox/issues/new/choose";
const DISCUSSIONS_URL: &str = "https://github.com/zealsprince/rox/discussions";
const CHAT_URL: &str = "https://hivecom.net/chat?channel=rox";

const MENU_MARGIN: Pixels = px(8.);

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
            MenuAction::AbRepeat => {
                self.state
                    .player
                    .update(cx, |player, cx| player.ab_mark(cx));
            }
            MenuAction::GoToTime => self.toggle_goto(window, cx),
            MenuAction::Sleep(pick) => {
                let after = pick.minutes().map(|m| Duration::from_secs(m * 60));
                self.state
                    .player
                    .update(cx, |player, cx| player.set_sleep(after, cx));
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
            MenuAction::OpenHealth => crate::health_window::open(self.state.clone(), cx),
            MenuAction::OpenPowerSearch => crate::search_window::open(self.state.clone(), cx),
            MenuAction::OpenQuickPlay => self.toggle_quick_play(window, cx),
            MenuAction::OpenConsole => crate::console_window::open(cx),
            MenuAction::OpenTasks => crate::tasks_window::open(cx),
            MenuAction::OpenEqualizer => crate::eq_window::open(cx),
            MenuAction::RescanLibrary => self.rescan_library(cx),
            // The passes go through the same start prompt as everywhere else.
            MenuAction::MeasureReplayGain => {
                self.start_pass_prompt(crate::pass_prompt::Pass::ReplayGain, cx)
            }
            MenuAction::AnalyzeTempo => self.start_pass_prompt(
                crate::pass_prompt::Pass::Tempo {
                    retry_refused: false,
                },
                cx,
            ),
            MenuAction::BuildAcoustic => {
                self.start_pass_prompt(crate::pass_prompt::Pass::Acoustic, cx)
            }
            MenuAction::FillSortNames => self.start_pass_prompt(
                crate::pass_prompt::Pass::SortNames {
                    scope: crate::sortnames_job::Scope::default(),
                },
                cx,
            ),
            MenuAction::RomanizeLibrary => {
                self.start_pass_prompt(crate::pass_prompt::Pass::Romanize, cx)
            }
            MenuAction::FindDuplicates => self.open_duplicates(cx),
            MenuAction::TagGenres => self.open_genre_tagger(cx),
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
                // Deferred: the teardown dumps every panel, this workspace included, and a
                // read inside its own update panics. A popped-out menu panel just closes.
                let ws = cx.entity();
                window.defer(cx, move |window, cx| {
                    if is_workspace_window(window, cx) {
                        close_workspace_window(Some(ws), window, cx);
                    }
                    window.remove_window();
                });
            }
            MenuAction::Quit => {
                // Quitting bypasses the close hook, so persist the layout and frame here.
                self.persist(window, cx);
                cx.quit();
            }
        }
    }

    /// A docked bar arms on a single clean Alt tap. A hidden bar needs the
    /// double tap, which pins it and arms it at once.
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

    /// The `MenuNav` context hands space and the arrows back from the playback
    /// bindings. Never arms on macOS, where the menus live in the system bar.
    fn set_menu_keys(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && !cfg!(target_os = "macos");
        if self.menubar_keys == on {
            return;
        }
        self.menubar_keys = on;
        if on {
            self.menu_top = self.open_menu.unwrap_or(0);
            if self.menubar_collapsed {
                self.menu_root = true;
            }
        } else {
            self.close_menus(cx);
        }
        cx.notify();
    }

    pub(crate) fn drop_menu_keys(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.menubar_keys {
            return false;
        }
        self.set_menu_keys(false, cx);
        true
    }

    pub(crate) fn cancel_alt_tap(&mut self) {
        self.alt_tap.cancel();
    }

    /// The keyboard mode goes too, or an armed bar would stay stranded over the
    /// dock with only Escape to clear it.
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

    /// True means the caller stops the event: the bar has the keyboard until
    /// Escape. Bindings beat key listeners, so `MenuNav` is what really takes
    /// space and the arrows (see `keymap::PLAYBACK`).
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
        // Shift doesn't count as a chord: the letters match either case.
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
        // Swallow everything else, so a stray key never reaches the panel below.
        true
    }

    fn menu_escape(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.menu_group_slot = None;
            cx.notify();
        } else if self.open_submenu.is_some() {
            self.open_flyout(None);
            cx.notify();
        } else if self.open_menu.is_some() && self.menu_root {
            self.show_top(None);
            cx.notify();
        } else if self.open_menu.is_some() {
            self.close_menus(cx);
        } else {
            self.set_menu_keys(false, cx);
            self.unpin_menubar(cx);
        }
    }

    fn menu_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.open_menu.is_none() && self.menu_root {
            self.menu_top = self.step_top(delta);
            cx.notify();
            return;
        }
        if self.open_menu.is_none() {
            self.open_top(self.menu_top, cx);
            if delta < 0 {
                self.menu_slot = self.menu_rows().last().map(|(slot, _)| *slot);
                self.menu_scroll_follow();
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
        self.menu_scroll_follow();
        cx.notify();
    }

    fn menu_out(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            self.open_subgroup = None;
            self.menu_group_slot = None;
        } else if self.open_submenu.is_some() {
            self.open_flyout(None);
        } else if self.menu_root {
            self.show_top(None);
        } else if self.open_menu.is_some() {
            self.open_top(self.step_top(-1), cx);
            return;
        } else {
            self.menu_top = self.step_top(-1);
        }
        cx.notify();
    }

    fn menu_in(&mut self, cx: &mut Context<Self>) {
        if self.open_subgroup.is_some() {
            return;
        }
        if self.open_submenu.is_some() {
            if let Some(NavRow::Open(group)) = self.row_at(self.flyout_rows(), self.menu_sub_slot) {
                self.open_subgroup = Some(group);
                self.reset_menu_scrolls(self.level(2));
                self.menu_group_slot = (!self.group_rows().is_empty()).then_some(0);
                self.menu_scroll_follow();
                cx.notify();
            }
            return;
        }
        if self.open_menu.is_some() {
            if let Some(NavRow::Open(entry)) = self.current_row() {
                self.open_flyout(Some(entry));
                self.menu_sub_slot = (!self.flyout_rows().is_empty()).then_some(0);
                self.menu_scroll_follow();
                cx.notify();
            } else if !self.menu_root {
                self.open_top(self.step_top(1), cx);
            }
            return;
        }
        if self.menu_root {
            self.open_top(self.menu_top, cx);
            return;
        }
        self.menu_top = self.step_top(1);
        cx.notify();
    }

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

    fn open_top(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close_menus(cx);
        self.menu_root = self.menubar_collapsed;
        self.menu_top = index;
        self.open_menu = Some(index);
        self.menu_slot = self.menu_rows().first().map(|(slot, _)| *slot);
        self.menu_scroll_follow();
        cx.notify();
    }

    fn show_top(&mut self, index: Option<usize>) {
        self.open_flyout(None);
        self.reset_menu_scrolls(self.level(0));
        self.open_menu = index;
        self.menu_slot = None;
        if let Some(index) = index {
            self.menu_top = index;
        }
    }

    fn step_top(&self, delta: isize) -> usize {
        let len = MENUS.len() as isize;
        (self.menu_top as isize + delta).rem_euclid(len) as usize
    }

    /// Leaving the letters up would keep eating the next window's keys.
    fn nav_run(&mut self, run: NavRun, window: &mut Window, cx: &mut Context<Self>) {
        self.set_menu_keys(false, cx);
        self.run_nav(run, window, cx);
    }

    /// The menu panel comes straight here, with no access letters to put away.
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

    fn row_at(&self, rows: Vec<NavRow>, at: Option<usize>) -> Option<NavRow> {
        nav_row_at(rows, at)
    }

    fn current_row(&self) -> Option<NavRow> {
        let slot = self.menu_slot?;
        self.menu_rows()
            .into_iter()
            .find(|(row, _)| *row == slot)
            .map(|(_, row)| row)
    }

    fn menu_rows(&self) -> Vec<(NavSlot, NavRow)> {
        self.open_menu.map(menu_entry_rows).unwrap_or_default()
    }

    fn flyout_rows(&self) -> Vec<NavRow> {
        match (self.open_menu, self.open_submenu) {
            (Some(menu), Some(entry)) => submenu_rows(menu, entry),
            _ => Vec::new(),
        }
    }

    fn group_rows(&self) -> Vec<NavRow> {
        self.open_subgroup.map(subgroup_rows).unwrap_or_default()
    }
}

pub(crate) fn nav_row_at(rows: Vec<NavRow>, at: Option<usize>) -> Option<NavRow> {
    rows.into_iter().nth(at?)
}

/// Headings and gated-off sections contribute no rows. Off the table
/// rather than menubar state, so the menu panel walks `MENUS` with the same
/// three functions and the keyboard can't disagree between the two.
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

/// Read from the same lists the flyout draws, so the two walk in step.
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
            // Group 0 is the presets when there are any, as the picker numbers them.
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
    fn nav_on(&self, entry: usize, row: Option<usize>) -> bool {
        self.menu_slot == Some((entry, row))
    }

    fn nav_sub(&self, row: usize) -> bool {
        self.menu_sub_slot == Some(row)
    }

    /// Addressed by place in the flyout, not by group number.
    fn nav_on_group(&self, group: usize) -> bool {
        matches!(
            self.row_at(self.flyout_rows(), self.menu_sub_slot),
            Some(NavRow::Open(open)) if open == group
        )
    }

    /// Group rows are built whether or not their group is open.
    fn nav_group(&self, group: usize, row: usize) -> bool {
        self.open_subgroup == Some(group) && self.menu_group_slot == Some(row)
    }

    /// A leave with a dropdown open is the pointer moving into the dropdown,
    /// so the pin holds through it.
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

    // `+ use<>` on these builders: they return owned elements while the caller
    // still holds `cx`, so the capture list is pinned empty.
    fn menu_button(
        &self,
        index: usize,
        menu: &'static Menu,
        letter: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_menu == Some(index);
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
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menus(cx)))
            })
            .child(menu_label(menu.label, letter))
            .when(open, |d| {
                d.child(Self::dropdown_at(self.dropdown(menu, cx)))
            })
    }

    fn collapsed_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let open = self.menu_root;
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
                cx.listener(|this, _, _, cx| {
                    let reopen = !this.menu_root;
                    this.close_menus(cx);
                    this.menu_root = reopen;
                }),
            )
            .when(open, |d| {
                d.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menus(cx)))
            })
            .child(
                svg()
                    .path(icons::MENU)
                    .size(px(14.))
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| d.child(Self::dropdown_at(self.root_menu(cx))))
    }

    fn root_menu(&self, cx: &mut Context<Self>) -> Div {
        let letters = self.menubar_keys.then(mnemonics).unwrap_or_default();
        let list = self
            .menu_list(0)
            .children(MENUS.iter().enumerate().map(|(i, menu)| {
                let letter = letters.get(i).cloned().flatten().map(|(range, _)| range);
                self.root_row(i, menu, letter, cx)
            }));
        self.menu_frame(0, px(160.), list)
            .child(self.menu_surface_capture(0, cx))
    }

    fn root_row(
        &self,
        index: usize,
        menu: &'static Menu,
        letter: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_menu == Some(index);
        let cursor = self.menubar_keys && self.menu_top == index;
        div()
            .id(("menu-root", index))
            .relative()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .cursor_pointer()
            .when(open || cursor, nav_lit)
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.open_menu != Some(index) {
                    this.show_top(Some(index));
                    cx.notify();
                }
            }))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .child(menu_label(menu.label, letter))
            .child(
                svg()
                    .path(icons::CHEVRON_RIGHT)
                    .size_3()
                    .text_color(palette::text_muted()),
            )
            .when(open, |d| {
                d.child(Self::flyout_at(
                    self.flyout_left(0),
                    self.dropdown(menu, cx),
                ))
            })
    }
    /// One builder, so the docked row and the alt-revealed overlay stay the same
    /// bar.
    pub(crate) fn menubar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let native_menus = cfg!(target_os = "macos");
        div()
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(px(MENU_BAR_H))
            .flex_none()
            .bg(palette::bg_menubar())
            .border_b_1()
            .border_color(palette::border())
            .when(!native_menus, |d| d.child(self.menubar_capture(cx)))
            .children(self.traffic_lights(window, cx))
            .children(self.mini_button(cx))
            .when(!native_menus && self.menubar_collapsed, |d| {
                d.child(self.collapsed_button(cx))
            })
            .when(!native_menus && !self.menubar_collapsed, |d| {
                // Only worked out while shown, so the common frame skips the lookups.
                let letters = self.menubar_keys.then(mnemonics).unwrap_or_default();
                d.children(MENUS.iter().enumerate().map(|(i, menu)| {
                    let letter = letters.get(i).cloned().flatten().map(|(range, _)| range);
                    self.menu_button(i, menu, letter, cx)
                }))
            })
            // A drag handle, so a decorations-off window still moves by its menu bar.
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
            )
            .child(self.library_status(window, cx))
    }

    /// Painted first so [`Self::menubar_fit_capture`] finds the bounds set.
    fn menubar_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        canvas(
            move |bounds, window, cx| {
                let viewport_h = window.viewport_size().height;
                view.update(cx, |this, _| {
                    this.menubar_bounds = Some(bounds);
                    this.menu_viewport_h = viewport_h;
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// Where it lands is where the row's content ends, which decides the fold.
    fn menubar_fit_capture(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = cx.entity();
        canvas(
            move |bounds, _, cx| {
                view.update(cx, |this, cx| this.note_menubar_fit(bounds.origin.x, cx));
            },
            |_, _, _, _| {},
        )
        .w_0()
        .h_full()
        .flex_none()
    }

    /// The unfolded width is remembered with some slack, so a resize can't
    /// flicker on the boundary.
    fn note_menubar_fit(&mut self, end_x: Pixels, cx: &mut Context<Self>) {
        let Some(bar) = self.menubar_bounds else {
            return;
        };
        let collapse = if self.menubar_collapsed {
            bar.size.width < self.menubar_need_w + px(8.)
        } else {
            self.menubar_need_w = end_x - bar.origin.x + tokens::SPACE_MD;
            self.menubar_need_w > bar.size.width + px(0.5)
        };
        if collapse != self.menubar_collapsed {
            self.menubar_collapsed = collapse;
            self.close_menus(cx);
        }
    }

    /// Only when this window draws its own chrome on macOS.
    fn traffic_lights(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !cfg!(target_os = "macos") || settings::os_decorations() {
            return None;
        }
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

    fn library_status(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
        let buttons = settings::menubar_buttons();
        // Summed on a pick rather than here, so the bar doesn't walk the
        // projection every frame. Only at idle; a scan's progress outranks it.
        let selection = idle.then_some(self.selection_status).flatten();
        let status = match selection {
            Some((picked, None)) => SharedString::from(format!(
                "{status} ({})",
                rox_i18n::t!("status-count-selected", count = picked as u64)
            )),

            Some((picked, Some(total_ms))) => SharedString::from(format!(
                "{status} ({} / {})",
                rox_i18n::t!("status-count-selected", count = picked as u64),
                rox_panel_api::group_head::fmt_total(total_ms)
            )),

            None => status,
        };
        // The status side shrinks and truncates before the bar folds the menus.
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink()
            .min_w_0()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            // The buttons only take the left button, so a right-click anywhere here
            // picks which buttons draw.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.open_buttons_menu(event.position, window, cx);
                }),
            )
            .when_some(crate::startup::updates::available(), |d, version| {
                d.child(self.update_chip(version, cx))
            })
            .when(!status.is_empty(), |d| {
                let library = self.state.library.clone();
                let picked = self.state.selection.clone();
                d.child(
                    div()
                        .id("library-status")
                        .flex_shrink()
                        .min_w(px(64.))
                        .max_w(px(480.))
                        .truncate()
                        .text_color(palette::text_muted())
                        .when(scanning, |d| d.text_xs())
                        .when(idle, |d| {
                            d.tooltip(move |_window, cx| {
                                if selection.is_some() {
                                    rox_panels::status::selection_tooltip(&library, &picked, cx)
                                } else {
                                    rox_panels::status::library_tooltip(&library, cx)
                                }
                            })
                        })
                        .child(status),
                )
            })
            .when_some(busy, |d, label| {
                // Tabular digits, so the ticking count never changes the badge width.
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
            // The abort stands in the rescan's slot while a scan runs, so it never
            // jumps.
            .when(buttons.rescan && can_rescan && idle, |d| {
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
            .when(buttons.tasks, |d| d.child(crate::tasks_window::control(cx)))
            .when(buttons.sleep, |d| d.child(self.sleep_control(cx)))
            // The occluding layer under the popup closes it on an outside click.
            .when_some(self.status_menu.as_ref(), |d, (at, menu, _)| {
                d.child(
                    deferred(
                        anchored().child(
                            div()
                                .w(window.bounds().size.width)
                                .h(window.bounds().size.height)
                                .occlude()
                                .child(
                                    anchored()
                                        .position(*at)
                                        .snap_to_window_with_margin(px(8.))
                                        .child(menu.clone()),
                                ),
                        ),
                    )
                    .with_priority(1),
                )
            })
            .when(!cfg!(target_os = "macos"), |d| {
                d.child(self.menubar_fit_capture(cx))
            })
    }

    fn sleep_control(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let left = sleep_minutes_left(
            self.state
                .player
                .read(cx)
                .sleep_remaining()
                .map(|left| left.as_secs()),
        );
        let (tip, color) = match left {
            Some(minutes) => (
                rox_i18n::t!("playback-sleep-tip-remaining", minutes = minutes),
                palette::accent(),
            ),
            None => (rox_i18n::t!("playback-sleep-tip"), palette::text_muted()),
        };
        panel::Tip::keyed("sleep-timer", tip).apply(
            div()
                .flex_none()
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .child(
                    svg()
                        .path(icons::MOON)
                        .size(px(12.))
                        .flex_none()
                        .text_color(color),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.open_sleep_menu(event.position, window, cx);
                    }),
                ),
        )
    }

    /// Dispatched through the menu action so the two entry points can't drift.
    fn open_sleep_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        const PICKS: [(&str, SleepPick); 4] = [
            ("playback-sleep-15", SleepPick::Min15),
            ("playback-sleep-30", SleepPick::Min30),
            ("playback-sleep-60", SleepPick::Min60),
            ("playback-sleep-90", SleepPick::Min90),
        ];
        let sleep = self
            .state
            .player
            .read(cx)
            .sleep_remaining()
            .map(|left| left.as_secs());
        let weak = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            let row = |label: SharedString, icon: &'static str, pick: SleepPick| {
                let weak = weak.clone();
                gpui_component::menu::PopupMenuItem::new(label)
                    .icon(Icon::default().path(icon))
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.run(MenuAction::Sleep(pick), window, cx))
                            .ok();
                    })
            };
            for (label, pick) in PICKS {
                menu = menu.item(row(rox_i18n::t_static(label).into(), icons::MOON, pick));
            }
            menu.separator()
                .item(row(sleep_off_label(sleep), icons::CLOCK, SleepPick::Off))
        });
        self.show_status_menu(at, menu, window, cx);
    }

    /// A flip lands in the live set and the look at once.
    fn open_buttons_menu(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = settings::menubar_buttons();
        type Field = fn(&mut MenubarButtons) -> &mut bool;
        let rows: [(&str, Field); 3] = [
            ("menubar-button-tasks", |b| &mut b.tasks),
            ("menubar-button-sleep", |b| &mut b.sleep),
            ("menubar-button-rescan", |b| &mut b.rescan),
        ];
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for (label, field) in rows {
                let mut probe = current;
                let checked = *field(&mut probe);
                menu = menu.item(
                    gpui_component::menu::PopupMenuItem::new(rox_i18n::t_static(label))
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let mut next = settings::menubar_buttons();
                            let slot = field(&mut next);
                            *slot = !*slot;
                            settings::set_menubar_buttons(next, cx);
                            Settings::update(move |s| {
                                s.look.bundle.appearance.menubar_buttons = next
                            });
                        }),
                );
            }
            menu
        });
        self.show_status_menu(at, menu, window, cx);
    }

    fn show_status_menu(
        &mut self,
        at: Point<Pixels>,
        menu: Entity<PopupMenu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.status_menu = None;
            cx.notify();
        });
        self.status_menu = Some((at, menu, subscription));
        cx.notify();
    }

    fn update_chip(&self, version: String, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
                    // The chip's text color doesn't reach the svg.
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

    fn menu_surface_capture(
        &self,
        level: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity();
        canvas(
            move |bounds, window, cx| {
                let viewport = window.viewport_size();
                view.update(cx, |this, _| {
                    this.menu_surfaces[level] = Some(bounds);
                    this.menu_viewport_w = viewport.width;
                    this.menu_viewport_h = viewport.height;
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

    /// Dropdowns count from 0 on the unfolded bar and from 1 on a collapsed
    /// one, where the root list sits underneath.
    fn level(&self, level: usize) -> usize {
        level + usize::from(self.menubar_collapsed)
    }

    /// Level 0 has the room below the bar; a flyout gets the whole window and
    /// `anchored` slides it to fit.
    fn menu_fit(&self, level: usize) -> Pixels {
        let border = px(2.);
        if level == 0 {
            let bar_bottom = self
                .menubar_bounds
                .map(|bar| bar.bottom())
                .unwrap_or(px(MENU_BAR_H));
            self.menu_viewport_h - bar_bottom - MENU_MARGIN - border
        } else {
            self.menu_viewport_h - MENU_MARGIN * 2. - border
        }
    }

    /// The first paint has no window height yet, so that frame runs uncapped.
    fn menu_list(&self, level: usize) -> Stateful<Div> {
        let fit = self.menu_fit(level);
        div()
            .id(("menu-list", level))
            .flex()
            .flex_col()
            .py(tokens::SPACE_XS)
            .when(fit > Pixels::ZERO, |d| d.max_h(fit))
            .overflow_y_scroll()
            .track_scroll(&self.menu_scrolls[level])
    }

    /// Anything else the caller adds goes on the frame, not in the list, so it
    /// doesn't count toward the scroll extent.
    fn menu_frame(&self, level: usize, min_w: Pixels, list: impl IntoElement) -> Div {
        div()
            .relative()
            .min_w(min_w)
            .flex()
            .flex_col()
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .child(list)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.menu_scrolls[level])),
            )
    }

    fn dropdown_at(frame: Div) -> impl IntoElement {
        deferred(
            div().absolute().left_0().top(px(MENU_BAR_H)).child(
                anchored()
                    .snap_to_window_with_margin(MENU_MARGIN)
                    .child(frame),
            ),
        )
    }

    /// Deferred to paint past the list's scroll clip, and anchored at the
    /// corner touching the row so it snaps back into the window. The top offset
    /// lines the first item up with the row.
    fn flyout_at(leftward: bool, frame: Div) -> impl IntoElement {
        let corner = if leftward {
            Corner::TopRight
        } else {
            Corner::TopLeft
        };
        deferred(
            flyout_side(div().absolute(), leftward).top(px(-5.)).child(
                anchored()
                    .anchor(corner)
                    .snap_to_window_with_margin(MENU_MARGIN)
                    .child(frame),
            ),
        )
    }

    /// The dropdown also draws headings, so its child index comes from
    /// [`dropdown_child_index`].
    fn menu_scroll_follow(&self) {
        let (level, child) = if self.open_subgroup.is_some() {
            (self.level(2), self.menu_group_slot)
        } else if self.open_submenu.is_some() {
            (self.level(1), self.menu_sub_slot)
        } else if let Some(menu) = self.open_menu {
            let child = self
                .menu_slot
                .and_then(|slot| MENUS.get(menu).map(|menu| dropdown_child_index(menu, slot)));
            (self.level(0), child)
        } else {
            return;
        };
        if let Some(child) = child {
            self.menu_scrolls[level].scroll_to_item(child);
        }
    }

    fn reset_menu_scrolls(&self, level: usize) {
        for scroll in &self.menu_scrolls[level..] {
            scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
        }
    }

    /// The rows are the list's direct children, in the order
    /// [`dropdown_child_index`] counts.
    fn dropdown(&self, menu: &'static Menu, cx: &mut Context<Self>) -> Div {
        let level = self.level(0);
        let list =
            self.menu_list(level)
                .children(menu.entries.iter().enumerate().flat_map(
                    |(i, entry)| -> Vec<AnyElement> {
                        match entry {
                            MenuEntry::Item(item) => vec![
                                self.action_item(*item, cx)
                                    .id(("menu-entry", i))
                                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                        if *hovered && this.open_submenu.is_some() {
                                            this.open_flyout(None);
                                            cx.notify();
                                        }
                                    }))
                                    .when(self.nav_on(i, None), nav_lit)
                                    .into_any_element(),
                            ],
                            MenuEntry::Section(label) => {
                                vec![menu_section(label).into_any_element()]
                            }
                            MenuEntry::Panels(section) if !section_shows(section) => Vec::new(),
                            MenuEntry::Panels(section) => match section.group {
                                None => section
                                    .panels
                                    .iter()
                                    .enumerate()
                                    .map(|(j, def)| {
                                        self.action_item(panel_menu_item(def), cx)
                                            .id(("panel-entry", j))
                                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                                if *hovered && this.open_submenu.is_some() {
                                                    this.open_flyout(None);
                                                    cx.notify();
                                                }
                                            }))
                                            .when(self.nav_on(i, Some(j)), nav_lit)
                                            .into_any_element()
                                    })
                                    .collect(),
                                Some((label, icon)) => vec![
                                    self.submenu_row(i, label, icon, section.panels, cx)
                                        .into_any_element(),
                                ],
                            },
                            MenuEntry::LayoutsSubmenu {
                                label,
                                icon,
                                target,
                                with_new,
                            } => vec![
                                self.layouts_submenu_row(i, label, icon, *target, *with_new, cx)
                                    .into_any_element(),
                            ],
                            MenuEntry::WorkspacesSubmenu {
                                label,
                                icon,
                                target,
                                with_new,
                            } => vec![
                                self.workspaces_submenu_row(i, label, icon, *target, *with_new, cx)
                                    .into_any_element(),
                            ],
                            MenuEntry::PresetsSubmenu {
                                label,
                                icon,
                                target,
                            } => vec![
                                self.presets_submenu_row(i, label, icon, *target, cx)
                                    .into_any_element(),
                            ],
                            MenuEntry::PanelWindowsSubmenu { label, icon } => vec![
                                self.panel_windows_submenu_row(i, label, icon, cx)
                                    .into_any_element(),
                            ],
                        }
                    },
                ));
        self.menu_frame(level, px(180.), list)
            .child(self.menu_surface_capture(level, cx))
    }

    fn action_item(&self, item: MenuItem, cx: &mut Context<Self>) -> Div {
        let action = item.action;
        let checked = match action {
            MenuAction::ToggleMenubar => settings::hide_menubar(),
            MenuAction::ToggleDesignMode => settings::design_mode(),
            MenuAction::ToggleDecorations => settings::os_decorations(),
            MenuAction::ToggleQuitToTray => settings::quit_to_tray(),
            MenuAction::ToggleArtTheming => palette::art_theming(),
            MenuAction::TogglePostShader => crate::workspace::post_shader_on(),
            _ => false,
        };
        let player = self.state.player.read(cx);
        let (is_playing, ab) = (player.is_playing(), player.ab_state());
        let sleep = player.sleep_remaining().map(|left| left.as_secs());
        let (label, icon) = menu_item_display(item, is_playing, ab, sleep);
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

    /// The flyout stays open until another entry is hovered, so the pointer
    /// can cross the gap.
    fn submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        panels: &'static [PanelDef],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                let level = self.level(1);
                let list = self
                    .menu_list(level)
                    .children(panels.iter().enumerate().map(|(row, def)| {
                        self.action_item(panel_menu_item(def), cx)
                            .when(self.nav_sub(row), nav_lit)
                    }));
                d.child(Self::flyout_at(
                    self.flyout_left(self.level(0)),
                    self.menu_frame(level, px(160.), list),
                ))
            })
    }

    fn layouts_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: LayoutTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                let presets = rox_core::settings::layouts::all(&Settings::load());
                let level = self.level(1);
                let mut flyout = self.menu_list(level);
                if with_new {
                    flyout = flyout.child(self.save_new_item(cx).when(self.nav_sub(0), nav_lit));
                }
                if presets.is_empty() {
                    // The Save flyout always has its New row.
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
                d.child(Self::flyout_at(
                    self.flyout_left(self.level(0)),
                    self.menu_frame(level, px(180.), flyout),
                ))
            })
    }

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

    fn presets_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: PanelTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_submenu == Some(index);
        let lit = open || self.nav_on(index, None);
        submenu_shell(index, label, icon, lit, cx).when(open, |d| {
            let presets = panel_presets::saved();
            let level = self.level(1);
            let list = self.menu_list(level);
            let list = if presets.is_empty() {
                list.child(flyout_note(rox_i18n::t!("menu-no-presets")))
            } else {
                list.children(presets.into_iter().enumerate().map(|(row, preset)| {
                    self.preset_item(preset, target, cx)
                        .when(self.nav_sub(row), nav_lit)
                }))
            };
            d.child(Self::flyout_at(
                self.flyout_left(self.level(0)),
                self.menu_frame(level, px(180.), list),
            ))
        })
    }

    /// Every pick opens a panel in its own window, which is why this is a
    /// flyout of its own rather than a target on the Panels menu.
    fn panel_windows_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.open_submenu == Some(index);
        let lit = open || self.nav_on(index, None);
        submenu_shell(index, label, icon, lit, cx).when(open, |d| {
            let presets = panel_presets::saved();
            let level = self.level(1);
            let mut flyout = self.menu_list(level);
            // Group 0 is the presets, so catalog groups start one along.
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
                    None => flyout.children(rows),
                    Some((label, icon)) => {
                        flyout.child(self.panel_window_group(i + 1, label, icon, rows, cx))
                    }
                };
            }
            // This flyout hosts the group flyouts, so it captures its own bounds.
            d.child(Self::flyout_at(
                self.flyout_left(self.level(0)),
                self.menu_frame(level, px(180.), flyout)
                    .child(self.menu_surface_capture(level, cx)),
            ))
        })
    }

    /// `index` is the group's slot in the picker, apart from the entry indices
    /// above.
    fn panel_window_group(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        rows: Vec<Div>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                    this.reset_menu_scrolls(this.level(2));
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
                let level = self.level(2);
                d.child(Self::flyout_at(
                    self.flyout_left(self.level(1)),
                    self.menu_frame(level, px(180.), self.menu_list(level).children(rows)),
                ))
            })
    }

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

    fn close_menus(&mut self, cx: &mut Context<Self>) {
        self.menu_root = false;
        self.open_menu = None;
        self.menu_slot = None;
        self.open_submenu = None;
        self.menu_sub_slot = None;
        self.open_subgroup = None;
        self.menu_group_slot = None;
        self.reset_menu_scrolls(0);
        cx.notify();
    }

    /// The keyboard cursor goes with it, so no highlight is left in a list it
    /// left.
    fn open_flyout(&mut self, index: Option<usize>) {
        self.open_submenu = index;
        self.reset_menu_scrolls(self.level(1));
        self.menu_sub_slot = None;
        self.open_subgroup = None;
        self.menu_group_slot = None;
    }

    fn workspaces_submenu_row(
        &self,
        index: usize,
        label: &'static str,
        icon: &'static str,
        target: WorkspaceTarget,
        with_new: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                // Read only once the flyout opens. The Save flyout can't overwrite shipped
                // bundles, so it drops them.
                let mut entries = crate::workspaces::all();
                if target == WorkspaceTarget::Overwrite {
                    entries.retain(|entry| !entry.builtin);
                }
                let level = self.level(1);
                let mut flyout = self.menu_list(level);
                if with_new {
                    flyout = flyout.child(
                        self.save_new_workspace_item(cx)
                            .when(self.nav_sub(0), nav_lit),
                    );
                }
                if entries.is_empty() {
                    // The Save flyout always has its New row.
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
                d.child(Self::flyout_at(
                    self.flyout_left(self.level(0)),
                    self.menu_frame(level, px(180.), flyout),
                ))
            })
    }

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

pub(crate) fn toggle_design_mode(cx: &mut App) {
    let on = !settings::design_mode();
    settings::set_design_mode(on, cx);
    Settings::update(move |s| s.design_mode = on);
    native_menu::rebuild(cx);
}

/// The flyout itself is chained on by the caller.
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

/// Walks the entries the way [`Workspace::dropdown`] draws them.
pub(crate) fn dropdown_child_index(menu: &Menu, slot: NavSlot) -> usize {
    let (entry, row) = slot;
    let before: usize = menu
        .entries
        .iter()
        .take(entry)
        .map(|entry| match entry {
            MenuEntry::Panels(section) if !section_shows(section) => 0,
            MenuEntry::Panels(section) if section.group.is_none() => section.panels.len(),
            _ => 1,
        })
        .sum();
    before + row.unwrap_or(0)
}

fn flyout_note(text: impl Into<SharedString>) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .text_color(palette::text_muted())
        .child(text.into())
}

/// The Alt tap tracker behind the menubar pin: two quick taps pin a hidden
/// bar up. A held Alt is awkward to click under: Alt+drag is the
/// compositor's window move, and macOS Option-click on the zoom light
/// zooms. Only a clean tap counts: Alt alone, released quickly.
#[derive(Default)]
pub(crate) struct AltTap {
    held_since: Option<Instant>,
    tapped_at: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AltTapKind {
    None,
    Tap,
    DoubleTap,
}

impl AltTap {
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

    fn cancel(&mut self) {
        self.held_since = None;
        self.tapped_at = None;
    }
}

/// Lands on the first row, or the last stepping backwards, when the cursor
/// isn't on one yet.
pub(crate) fn step_index(at: Option<usize>, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(at) = at else {
        return Some(if delta < 0 { len - 1 } else { 0 });
    };
    Some((at as isize + delta).rem_euclid(len as isize) as usize)
}

/// First free letter of the translated label, so the underline is the key
/// you press in any locale. None once a label runs out; that menu keeps
/// the arrows.
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

/// gpui hands letters over as one-character keys, so anything longer
/// matches nothing.
fn mnemonic_menu(key: &str) -> Option<usize> {
    let mut chars = key.chars();
    let pressed = chars.next().filter(|_| chars.next().is_none())?;
    mnemonics()
        .iter()
        .position(|hit| hit.as_ref().is_some_and(|(_, c)| *c == pressed))
}

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

/// The same wash hover paints, so pointer and arrows share one cursor.
pub(crate) fn nav_lit<T: Styled>(d: T) -> T {
    d.bg(palette::bg_control_hover_opaque())
}

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

    /// A translation may run a label out of free letters; the source locale
    /// must not.
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
        // "escape" starting with an e must not open the e menu.
        assert_eq!(mnemonic_menu("escape"), None);
        assert_eq!(mnemonic_menu("enter"), None);
    }

    #[test]
    fn dropdown_children_run_with_the_rows() {
        for (i, menu) in MENUS.iter().enumerate() {
            let mut last = None;
            for (slot, _) in menu_entry_rows(i) {
                let child = dropdown_child_index(menu, slot);
                assert!(
                    last.is_none_or(|last| child > last),
                    "{}: row {slot:?} landed on child {child} behind {last:?}",
                    menu.label
                );
                last = Some(child);
            }
        }
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        assert_eq!(step_index(Some(2), 1, 3), Some(0));
        assert_eq!(step_index(Some(0), -1, 3), Some(2));
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
        // The pair is consumed when it fires.
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
        let base = Instant::now();
        let mut tracker = AltTap::default();
        assert_eq!(tap(&mut tracker, base, 0, 1000), AltTapKind::None);
        assert_eq!(tap(&mut tracker, base, 1100, 1150), AltTapKind::Tap);
    }

    #[test]
    fn a_chord_is_not_a_tap() {
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
