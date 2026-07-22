//! The Workspace settings page: the workspaces and presets sharing hub,
//! the composition tree of the opening window's dock, and the confirm
//! dialog for overwrites and applies. `impl SettingsWindow` methods in a
//! child module, reaching back into the window's private state.

use super::*;

impl SettingsWindow {
    /// The Workspace page: the sharing hub. A workspace is a whole look -
    /// layout presets, palette, appearance - traded as one file; presets are
    /// single layouts under it. The composition tree below shows the opening
    /// window's dock, splits and tab groups as muted structure lines, panels
    /// as named rows with their settings a click away.
    pub(crate) fn workspace_page(&self, cx: &mut Context<Self>) -> Div {
        let live = self.workspace.upgrade().is_some();
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "The window's panels as they sit in splits and tab groups; \
                 the arrows reorder a row among its siblings, the lock pins \
                 a panel in place, and the gear opens its settings",
            ),
        );
        match self.workspace.upgrade() {
            Some(workspace) => {
                let root = workspace.read(cx).dock().read(cx).items().view();
                let mut rows = Vec::new();
                self.tree_rows(root, 0, TreeSlot::Root, &mut rows, cx);
                body = body.child(div().flex().flex_col().children(rows));
            }
            None => {
                body = body.child(
                    div()
                        .text_color(palette::text_muted())
                        .child("The workspace window is closed"),
                );
            }
        }

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(self.workspaces_section(live, cx))
            .child(self.presets_section(live, cx))
            .child(section("Composition", None, body))
    }

    /// The workspaces section: the saved and shipped bundles as a list, each
    /// a whole look to apply, export, or delete. Saving the current state as
    /// a named workspace, and importing one, ride the header.
    fn workspaces_section(&self, live: bool, cx: &mut Context<Self>) -> Div {
        let settings = Settings::load();
        let entries = crate::workspaces::all(&settings);

        // Save-current-as and import ride the header, so a workspace is one
        // name away and a shared file one pick away.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.workspace_name).small().w(px(150.)))
            .child(small_button(
                "Save Current",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.save_workspace(window, cx)),
            ))
            .child(small_button(
                "Import",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_workspace(window, cx)),
            ));

        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "A workspace is a whole look - layouts, palette, appearance; \
                 applying one replaces all three",
            ),
        );
        if entries.is_empty() {
            list = list.child(
                div()
                    .text_color(palette::text_muted())
                    .child("No workspaces yet"),
            );
        } else {
            list = list.child(
                div().flex().flex_col().children(
                    entries
                        .into_iter()
                        .map(|entry| self.workspace_row(entry, live, cx)),
                ),
            );
        }
        section("Workspaces", Some(controls.into_any_element()), list)
    }

    /// One workspace's row: its name, a shipped tag when it comes from the
    /// app's assets, apply, and for the user's own, export, overwrite and
    /// delete.
    fn workspace_row(
        &self,
        entry: crate::workspaces::Entry,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = entry.bundle.name.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name.clone())),
            )
            .when(entry.builtin, |d| d.child(shipped_tag()))
            // Applying replaces the whole look, so it routes through the
            // confirm dialog rather than acting straight off the click.
            .child(small_button("Apply", icons::CHECK, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::ApplyWorkspace(name.clone()));
                    cx.notify();
                })
            }))
            .when(!entry.builtin, |d| {
                // Export, overwrite and delete are the user's own workspaces
                // only; a shipped one already lives in the app's assets, so
                // there's nothing to save back out. Overwrite routes through
                // the confirm dialog before the replace, matching the presets
                // list and unlike apply and delete which are their own undo.
                d.child(small_button("Export", icons::UPLOAD, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.export_workspace(&name, cx))
                }))
                .child(small_button("Overwrite", icons::REFRESH_CW, !live, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.pending = Some(Pending::OverwriteWorkspace(name.clone()));
                        cx.notify();
                    })
                }))
                .child(icon_button(icons::TRASH, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.delete_workspace(&name, cx))
                }))
            })
            .into_any_element()
    }

    /// The presets section: the saved and shipped layouts as a list, each
    /// with the roles the mini-player button toggles between and the ways
    /// to apply, delete, or overwrite it. Saving the live layout as a named
    /// preset rides the header.
    fn presets_section(&self, live: bool, cx: &mut Context<Self>) -> Div {
        let settings = Settings::load();
        let presets = crate::settings::layouts::all(&settings);

        // Save-current-as and import ride the header, so a preset is one
        // arrangement plus a name away, or one shared file away.
        let save = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.layout_name).small().w(px(150.)))
            .child(small_button(
                "Save Current",
                icons::DOWNLOAD,
                !live,
                cx.listener(|this, _, window, cx| this.save_layout_preset(window, cx)),
            ))
            .child(small_button(
                "Import",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_preset(window, cx)),
            ));

        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "Primary and mini are the two the menubar's mini-player button \
                 swaps between",
            ),
        );
        if presets.is_empty() {
            list = list.child(
                div()
                    .text_color(palette::text_muted())
                    .child("No layouts yet"),
            );
        } else {
            list = list.child(
                div().flex().flex_col().children(
                    presets
                        .into_iter()
                        .map(|preset| self.preset_row(preset, live, cx)),
                ),
            );
        }
        section("Layouts", Some(save.into_any_element()), list)
    }

    /// One preset's row: its name, a shipped tag when it comes from the
    /// app's assets, the primary and mini role badges, and apply plus, for
    /// the user's own, delete.
    fn preset_row(&self, preset: Preset, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let is_primary = self.primary_layout.as_deref() == Some(preset.name.as_str());
        let is_mini = self.mini_layout.as_deref() == Some(preset.name.as_str());
        let name = preset.name.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(preset.name.clone())),
            )
            .child(role_chip("Primary", is_primary, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.set_primary(&name, cx))
            }))
            .child(role_chip("Mini", is_mini, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.set_mini(&name, cx))
            }))
            .child(small_button("Apply", icons::CHECK, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.apply_preset(&name, cx))
            }))
            .child(small_button("Export", icons::UPLOAD, false, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.export_preset(&name, cx))
            }))
            // Overwrite the saved preset with the live layout; the dialog
            // confirms before the replace, unlike apply and delete which are
            // their own undo.
            .child(small_button("Overwrite", icons::REFRESH_CW, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::OverwritePreset(name.clone()));
                    cx.notify();
                })
            }))
            .child(icon_button(icons::TRASH, false, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.delete_preset(&name, cx))
            }))
            .into_any_element()
    }

    /// Save the workspace's live layout as a named preset, panel configs
    /// and themes along with it. An empty name is ignored; a name that
    /// already exists routes through the confirm dialog rather than a silent
    /// replace. Clears the field on a fresh save.
    fn save_layout_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let name = self.layout_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        if Settings::load().layouts.iter().any(|l| l.name == name) {
            self.pending = Some(Pending::OverwritePreset(name));
            cx.notify();
            return;
        }
        let dump = workspace.read(cx).dock().read(cx).dump(cx);
        let Ok(dump) = serde_json::to_value(dump) else {
            return;
        };
        let size = self.workspace_window_size(cx);
        Settings::update(move |s| s.layouts.push(NamedLayout { name, dump, size }));
        self.layout_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Replace the pending preset's dump and window size with the live ones,
    /// the confirm dialog's yes. Clears the name field on success.
    fn overwrite_preset(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let dump = workspace.read(cx).dock().read(cx).dump(cx);
            if let Ok(dump) = serde_json::to_value(dump) {
                let size = self.workspace_window_size(cx);
                Settings::update(move |s| {
                    if let Some(existing) = s.layouts.iter_mut().find(|l| l.name == name) {
                        existing.dump = dump;
                        existing.size = size;
                    }
                });
            }
        }
        self.layout_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// The workspace window's content size, for storing with a preset. None
    /// when that window is gone.
    fn workspace_window_size(&self, cx: &mut App) -> Option<LayoutSize> {
        self.workspace_window
            .update(cx, |_, window, _| {
                let s = window.window_bounds().get_bounds().size;
                LayoutSize {
                    width: s.width.into(),
                    height: s.height.into(),
                }
            })
            .ok()
    }

    /// Apply a preset to the workspace's dock, in its own window - the same
    /// path an imported file takes.
    fn apply_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let name = name.to_string();
        self.workspace_window
            .update(cx, |_, window, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.apply_named_layout(&name, window, cx);
                    });
                }
            })
            .ok();
        cx.notify();
    }

    /// Point the mini-player button's primary role at a preset, or clear it
    /// when the preset already holds the role.
    fn set_primary(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.primary_layout.as_deref() == Some(name);
        self.primary_layout = (!clear).then(|| name.to_string());
        let value = self.primary_layout.clone();
        Settings::update(move |s| s.primary_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Point the mini role at a preset, or clear it when the preset already
    /// holds it.
    fn set_mini(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.mini_layout.as_deref() == Some(name);
        self.mini_layout = (!clear).then(|| name.to_string());
        let value = self.mini_layout.clone();
        Settings::update(move |s| s.mini_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Delete a user preset, dropping any role it held so the button never
    /// points at a gone name.
    fn delete_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        if self.primary_layout.as_deref() == Some(name.as_str()) {
            self.primary_layout = None;
        }
        if self.mini_layout.as_deref() == Some(name.as_str()) {
            self.mini_layout = None;
        }
        Settings::update(|s| {
            s.layout_edits.remove(name.as_str());
            s.layouts.retain(|l| l.name != name);
            if s.primary_layout.as_deref() == Some(name.as_str()) {
                s.primary_layout = None;
            }
            if s.mini_layout.as_deref() == Some(name.as_str()) {
                s.mini_layout = None;
            }
        });
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Push the current roles to the workspace so its mini-player button
    /// reflects the edit without waiting on a reload, and repaint it.
    fn sync_roles_to_workspace(&self, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            let primary = self.primary_layout.clone();
            let mini = self.mini_layout.clone();
            workspace.update(cx, |workspace, cx| {
                workspace.set_mini_roles(primary, mini);
                cx.notify();
            });
        }
    }

    /// The confirm dialog, up while a destructive action waits on the user:
    /// an overwrite or a workspace apply, each with its own wording. A scrim
    /// occludes the page under it; the buttons are the only way out, no
    /// click-away, so the action is deliberate.
    pub(crate) fn confirm_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (title, body, confirm): (String, &'static str, &'static str) =
            match self.pending.as_ref()? {
                Pending::OverwritePreset(name) => (
                    format!("Overwrite \"{name}\"?"),
                    "This replaces the saved layout with the current one.",
                    "Overwrite",
                ),
                Pending::OverwriteWorkspace(name) => (
                    format!("Overwrite workspace \"{name}\"?"),
                    "This replaces the saved workspace with the current state.",
                    "Overwrite",
                ),
                Pending::ApplyWorkspace(name) => (
                    format!("Apply \"{name}\"?"),
                    "This replaces your layouts, palette, and appearance with the workspace's.",
                    "Apply",
                ),
            };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .w(px(320.))
                        .p(tokens::SPACE_MD)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(div().child(SharedString::from(title)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(body),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(tokens::SPACE_SM)
                                .child(dialog_button(
                                    "Cancel",
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(dialog_button(
                                    confirm,
                                    true,
                                    cx.listener(|this, _, window, cx| {
                                        this.confirm_pending(window, cx)
                                    }),
                                )),
                        ),
                ),
        )
    }

    /// Carry out the pending action, the confirm dialog's yes, and clear it.
    fn confirm_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.take() {
            Some(Pending::OverwritePreset(name)) => self.overwrite_preset(name, window, cx),
            Some(Pending::OverwriteWorkspace(name)) => self.overwrite_workspace(name, window, cx),
            Some(Pending::ApplyWorkspace(name)) => self.apply_workspace(&name, window, cx),
            None => {}
        }
    }

    /// One node of the dock into rows. Walks the live stack and tab
    /// entities rather than the dock's `DockItem` tree, which goes stale
    /// once tabs are dragged around; these are what `dump` serializes.
    /// `slot` carries where the node sits among its siblings, so its row
    /// can offer the reorder arrows.
    fn tree_rows(
        &self,
        node: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let view = node.view();
        if let Ok(stack) = view.clone().downcast::<StackPanel>() {
            let (axis, children) = {
                let stack = stack.read(cx);
                (stack.axis(), stack.panels().to_vec())
            };
            rows.push(chrome_row(
                depth,
                match axis {
                    Axis::Horizontal => "Split, side by side",
                    Axis::Vertical => "Split, stacked",
                },
                self.move_controls(&slot, cx),
            ));
            let len = children.len();
            for (ix, child) in children.into_iter().enumerate() {
                let child_slot = TreeSlot::Stack {
                    stack: stack.clone(),
                    ix,
                    len,
                };
                self.tree_rows(child, depth + 1, child_slot, rows, cx);
            }
            return;
        }
        if let Ok(tabs) = view.downcast::<TabPanel>() {
            let children = tabs.read(cx).panels().to_vec();
            // A group of one reads as just its panel; the group only
            // earns its own line once there are tabs to speak of. The
            // solo row inherits the group's slot, so its arrows move the
            // enclosing tab group within the split.
            if let [only] = children.as_slice() {
                self.panel_rows(only.clone(), depth, slot, rows, cx);
                return;
            }
            rows.push(chrome_row(depth, "Tabs", self.move_controls(&slot, cx)));
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

    /// A panel's row, and under a composite host (group, depth, slide)
    /// its hosted children as indented rows of their own, so the tree
    /// shows what the host holds instead of one opaque line.
    fn panel_rows(
        &self,
        panel: Arc<dyn PanelView>,
        depth: usize,
        slot: TreeSlot,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let children = crate::composite::hosted_children(&panel, cx);
        rows.push(self.panel_row(panel, depth, slot, cx));
        if let Some(children) = children {
            for child in children {
                match child {
                    Some(child) => {
                        rows.push(self.panel_row(child, depth + 1, TreeSlot::Hosted, cx))
                    }
                    None => rows.push(chrome_row(depth + 1, "Empty slot", None)),
                }
            }
        }
    }

    /// A panel's row of the tree: its name (the rename first with the
    /// type in parens), the reorder arrows, the placement-lock toggle,
    /// and the gear opening the same settings window the panel's own
    /// dropdown does. Hosted children skip the arrows and the lock: the
    /// dock never sees them, so neither applies.
    fn panel_row(
        &self,
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
                            if locked { icons::LOCK } else { icons::LOCK_OPEN },
                            false,
                            cx.listener(move |_, _, _, cx| {
                                panel_settings::toggle_locked_for_view(&lock_panel, cx);
                                cx.notify();
                            }),
                        );
                        // A closed lock is state worth seeing at rest;
                        // the open one only shows with the row's other
                        // controls.
                        d.child(if locked { button } else { reveal(button) })
                    })
                    .child(reveal(icon_button(icons::SETTINGS, false, move |_, _, cx| {
                        panel_settings::open_for_view(&panel, cx);
                    }))),
            )
            .into_any_element()
    }

    /// The move controls for a movable tree node: the lift-out arrow
    /// pulling it up a layer, then up and down among its siblings, inert
    /// where a direction has nowhere to go. None for the dock root and
    /// hosted children, which have no siblings to move among here.
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

    /// The lift-out arrow: pull the node one layer up. A tab leaves its
    /// group for one of its own beside it; a split's child (a tab group
    /// or nested split) moves out into the enclosing split. Inert where
    /// there is no layer above - the root split's children stay put.
    fn lift_button(&self, slot: &TreeSlot, cx: &mut Context<Self>) -> Div {
        match slot {
            TreeSlot::Stack { stack, ix, .. } => {
                let dock = self
                    .workspace
                    .upgrade()
                    .map(|workspace| workspace.read(cx).dock().downgrade());
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
            }
            TreeSlot::Root | TreeSlot::Hosted => div(),
        }
    }

    /// One reorder arrow: moves the node from its index to `to_ix` in
    /// its parent stack or tab group. The move APIs ignore out-of-range
    /// indices, but the ends render inert anyway so the tree telegraphs
    /// where a row can still go.
    fn move_button(
        &self,
        slot: &TreeSlot,
        icon: &'static str,
        inert: bool,
        to_ix: usize,
        cx: &mut Context<Self>,
    ) -> Div {
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
            }
            TreeSlot::Root | TreeSlot::Hosted => div(),
        }
    }

    /// Export a preset to a file: its dump, panel configs and themes
    /// included, so a single layout can leave as a shareable artifact. Works
    /// for shipped presets too, which are dumps like any other.
    fn export_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(preset) = crate::settings::layouts::resolve(&Settings::load(), name) else {
            return;
        };
        let dump = preset.dump;
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.json");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&dump) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    /// Pick a layout file and add it as a new preset, named after the file
    /// and deduped so an import never shadows an existing preset. The file
    /// must parse as a dock dump, the same shape export writes; anything else
    /// is ignored.
    fn import_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Some(dump) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .filter(|value| serde_json::from_value::<DockAreaState>(value.clone()).is_ok())
            else {
                return;
            };
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "imported".to_string());
            let name = crate::workspaces::unique_name(&stem, |candidate| {
                crate::settings::layouts::all(&Settings::load())
                    .iter()
                    .any(|p| p.name == candidate)
            });
            Settings::update(move |s| {
                s.layouts.push(NamedLayout {
                    name,
                    dump,
                    size: None,
                })
            });
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Flush the workspace window's live dock to the settings file. Panel
    /// config like the library's column arrangement only reaches disk on the
    /// next layout dump, so without this a workspace save from here would
    /// capture whatever's stale on disk instead of the current look.
    fn flush_workspace_layout(&self, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let _ = self.workspace_window.update(cx, |_, window, cx| {
            if let Some(ws) = ws.upgrade() {
                ws.update(cx, |this, cx| this.persist(window, cx));
            }
        });
    }

    /// Save the current state as a named workspace: layouts, palette, and
    /// appearance in one bundle. An empty name is ignored; a name that already
    /// exists routes through the confirm dialog. Clears the field on a fresh
    /// save.
    fn save_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.workspace_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.flush_workspace_layout(cx);
        if Settings::load().workspaces.iter().any(|w| w.name == name) {
            self.pending = Some(Pending::OverwriteWorkspace(name));
            cx.notify();
            return;
        }
        let bundle = WorkspaceBundle::from_settings(name, &Settings::load());
        Settings::update(move |s| s.workspaces.push(bundle));
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Replace a saved workspace with the current state, the confirm dialog's
    /// yes. Clears the name field.
    fn overwrite_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_workspace_layout(cx);
        let bundle = WorkspaceBundle::from_settings(name.clone(), &Settings::load());
        Settings::update(move |s| {
            if let Some(existing) = s.workspaces.iter_mut().find(|w| w.name == name) {
                *existing = bundle;
            }
        });
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// Delete a user workspace. Shipped ones carry no delete.
    fn delete_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        Settings::update(move |s| s.workspaces.retain(|w| w.name != name));
        cx.notify();
    }

    /// Export a workspace bundle to a file, the whole look as one shareable
    /// artifact. Works for shipped bundles too.
    fn export_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(bundle) = crate::workspaces::resolve(&Settings::load(), name) else {
            return;
        };
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.json");
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&bundle) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    /// Pick a workspace file and add it to the collection, named after the
    /// file when the bundle carries no name of its own and deduped so an
    /// import never shadows an existing workspace. A bundle from a newer
    /// format, or a file that isn't a bundle, is ignored.
    fn import_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Some(bundle) = crate::workspaces::read_bundle(&path, &Settings::load()) else {
                return;
            };
            Settings::update(move |s| s.workspaces.push(bundle));
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Apply a workspace: replace the live look wholesale, through the
    /// workspace's own apply so the persist, the active-layout guard, and
    /// the no-layout fallback to the default arrangement all ride one flow.
    /// This window only mirrors the applied look into its own editor state
    /// on top.
    fn apply_workspace(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bundle) = crate::workspaces::resolve(&Settings::load(), name) else {
            return;
        };
        let workspace = self.workspace.clone();
        let name = name.to_string();
        let applied = self
            .workspace_window
            .update(cx, |_, window, cx| {
                workspace.upgrade().is_some_and(|workspace| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.apply_workspace(&name, window, cx);
                    });
                    true
                })
            })
            .unwrap_or(false);
        // The workspace window can be gone with this one still open; the
        // look still applies and persists, there is just no dock to swap.
        if !applied {
            crate::workspaces::apply_look(&bundle, cx);
        }
        // Mirror the applied look into this window's own editor state so the
        // swatches, pickers, and sliders show it. apply_palette re-sets the
        // live palette, which the apply above already did; the repeat is
        // idempotent.
        self.apply_palette(Palette::from_map(&bundle.palette), window, cx);
        let a = &bundle.appearance;
        self.surface_opacity = a.surface_opacity;
        self.backdrop_strength = a.backdrop_strength;
        self.frame = a.frame;
        self.art_theming = a.art_theming;
        self.keep_dark = a.keep_dark;
        self.rating_style = a.rating_style;
        // The mini-player roles; the workspace's apply already moved its own
        // live copy along with the dock.
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        cx.notify();
    }

}
