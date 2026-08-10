//! The Workspace settings page: the workspaces and presets sharing hub,
//! the composition tree of the opening window's dock, and the confirm
//! dialog for overwrites and applies. `impl SettingsWindow` methods in a
//! child module, reaching back into the window's private state.

use super::*;

/// The card open under a workspace row: which workspace it belongs to, the
/// card as the file carries it, and an input per line somebody can type in.
/// Built when a row's details open and dropped when they close, so nothing
/// here outlives the workspace it describes.
pub(crate) struct CardEditor {
    /// The workspace the card belongs to, which is also the name its file is
    /// under.
    name: String,
    /// The card the bundle arrived with. What the dates read out, and the
    /// whole readout for a shipped bundle.
    meta: WorkspaceMeta,
    /// One input per line of [`CARD_FIELDS`], in that order. None for a
    /// shipped bundle: its file lives in the app's assets, where there's
    /// nothing to write back to, so its card is a readout.
    fields: Option<Vec<Entity<InputState>>>,
}

/// The card's editable lines: what each one is called, what an empty one
/// hints at, and the field it reads and writes. Created and updated stay
/// out of this list because nobody types a date; a save stamps them.
type CardField = (
    &'static str,
    &'static str,
    fn(&mut WorkspaceMeta) -> &mut String,
);
const CARD_FIELDS: [CardField; 5] = [
    ("Author", "Who made it", |meta| &mut meta.author),
    ("Description", "What the look is going for", |meta| {
        &mut meta.description
    }),
    ("Website", "Where it lives", |meta| &mut meta.website),
    (
        "Version",
        "Your own version, whatever you count in",
        |meta| &mut meta.version,
    ),
    ("License", "The terms you share it under", |meta| {
        &mut meta.license
    }),
];

/// How big an exported bundle gets before the export says something about
/// it. A look is text and a few thousand lines of WGSL until it carries
/// image assets, and those are what push a file into the megabytes. Soft on
/// purpose: it's a note in the log, never a refusal.
const EXPORT_SIZE_WARN: usize = 4 * 1024 * 1024;

impl CardEditor {
    /// What's typed in, as a card. The dates ride through untouched: they
    /// belong to the bundle's history, not to this form.
    fn typed(&self, cx: &App) -> WorkspaceMeta {
        let mut meta = self.meta.clone();
        let Some(fields) = self.fields.as_ref() else {
            return meta;
        };
        for ((_, _, field), input) in CARD_FIELDS.iter().zip(fields) {
            *field(&mut meta) = input.read(cx).value().trim().to_string();
        }
        meta
    }
}

impl SettingsWindow {
    /// The Workspace page: the sharing hub. A workspace is a whole look -
    /// layout presets, palette, appearance - traded as one file; presets are
    /// single layouts under it. The composition tree below shows the opening
    /// window's dock, splits and tab groups as muted structure lines, panels
    /// as named rows with their settings a click away.
    pub(crate) fn workspace_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let live = self.workspace.upgrade().is_some();
        PageBody::new()
            .section(self.workspaces_section(q, live, cx))
            .section(self.presets_section(q, live, cx))
            .section(self.panel_presets_section(q, cx))
            // The tree walks the live dock, so it only builds once the
            // query keeps it.
            .section(Section::new(
                q,
                icons::LAYOUT_DASHBOARD,
                "Composition",
                None,
                |rows| {
                    rows.custom(
                        &["dock", "panels", "tree", "splits", "tabs", "layout"],
                        || {
                            let mut body =
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(tokens::SPACE_XS)
                                    .child(div().text_xs().text_color(palette::text_muted()).child(
                                    "The window's panels as they sit in splits and tab groups; \
                                 the arrows reorder a row among its siblings, the lock pins \
                                 a panel in place, and the gear opens its settings",
                                ));
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
                            body.into_any_element()
                        },
                    )
                },
            ))
    }

    /// The workspaces section: the saved and shipped bundles as a list, each
    /// a whole look to apply, export, or delete. Saving the current state as
    /// a named workspace, and importing one, ride the header.
    fn workspaces_section(&self, q: &Query, live: bool, cx: &mut Context<Self>) -> Section {
        let entries = crate::workspaces::all();

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

        Section::new(
            q,
            icons::APP_WINDOW,
            "Workspaces",
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(
                    &["look", "bundle", "theme", "import", "export", "apply"],
                    || {
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
                            // A row and, for the one whose details are open,
                            // its card right under it, so the fields sit with
                            // the workspace they belong to.
                            list = list.child(
                                div().flex().flex_col().children(
                                    entries
                                        .into_iter()
                                        .flat_map(|entry| {
                                            let open = self
                                                .workspace_card
                                                .as_ref()
                                                .is_some_and(|card| card.name == entry.name);
                                            let row = self.workspace_row(entry, live, cx);
                                            [Some(row), open.then(|| self.workspace_card_body(cx))]
                                        })
                                        .flatten(),
                                ),
                            );
                        }
                        list.into_any_element()
                    },
                )
            },
        )
    }

    /// One workspace's row: its name with the author under it when the card
    /// names one, a shipped tag when it comes from the app's assets, the
    /// details toggle, apply, and for the user's own, export, overwrite and
    /// delete.
    fn workspace_row(
        &self,
        entry: crate::workspaces::Entry,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = entry.name.clone();
        // A shipped entry carries its author from the parse that built the
        // list; a saved one comes out of the authors read this window holds.
        let author = entry
            .author
            .clone()
            .or_else(|| self.workspace_authors.get(&name).cloned());
        let open = self
            .workspace_card
            .as_ref()
            .is_some_and(|card| card.name == name);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            // The card hangs under the row rather than in a window of its
            // own: it's a handful of lines about the workspace right there,
            // and only one is open at a time. The chevron leads the row so
            // it points at the name it expands, and so the disclosure sits
            // apart from the buttons that act on the workspace.
            .child(icon_button(
                if open {
                    icons::CHEVRON_DOWN
                } else {
                    icons::CHEVRON_RIGHT
                },
                false,
                {
                    let name = name.clone();
                    let builtin = entry.builtin;
                    cx.listener(move |this, _, window, cx| {
                        this.toggle_workspace_card(&name, builtin, window, cx)
                    })
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(SharedString::from(name.clone())))
                    .when_some(author, |d, author| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(SharedString::from(format!("by {author}"))),
                        )
                    }),
            )
            .when(entry.builtin, |d| d.child(shipped_tag()))
            // Applying replaces the whole look, so it routes through the
            // confirm dialog rather than acting straight off the click.
            .child(small_button("Apply", icons::CHECK, !live, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::ApplyWorkspace {
                        card: crate::workspaces::ApplyCard::for_name(&name),
                        imported: false,
                    });
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

    /// The open workspace's card, hanging under its row: the author's own
    /// lines about the look, editable for a saved workspace and a readout
    /// for a shipped one, over the dates a save stamped.
    fn workspace_card_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(card) = self.workspace_card.as_ref() else {
            return div().into_any_element();
        };
        let muted = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
        };
        // Indented to where the row's name starts, clear of the chevron that
        // opened it: the icon button plus the gap behind it.
        let mut body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .pb(tokens::SPACE_SM)
            .pl(px(14.) + tokens::SPACE_XS * 2. + tokens::SPACE_SM);
        match card.fields.as_ref() {
            Some(fields) => {
                body = body.child(muted(
                    "The card travels inside the file, so whoever you share this look \
                     with sees it"
                        .into(),
                ));
                for ((label, _, _), input) in CARD_FIELDS.iter().zip(fields) {
                    body = body.child(panel::setting_row(
                        label,
                        None,
                        Input::new(input).small().w(px(240.)),
                    ));
                }
                body = body.child(div().flex().flex_row().justify_end().child(small_button(
                    "Save Card",
                    icons::CHECK,
                    false,
                    cx.listener(|this, _, _, cx| this.save_workspace_card(cx)),
                )));
            }
            // A shipped bundle's file is in the app's assets, so there's
            // nothing to write back to. Fork it with Save Current under a
            // name of your own and the copy's card is yours to fill in.
            None if card.meta.is_empty() => {
                body = body.child(muted("This workspace carries no card".into()));
            }
            None => {
                for (label, _, field) in CARD_FIELDS {
                    let mut meta = card.meta.clone();
                    let value = field(&mut meta).clone();
                    if value.trim().is_empty() {
                        continue;
                    }
                    body = body.child(card_readout_line(label, value));
                }
            }
        }
        // The dates are the bundle's own history: a save stamps them, so
        // they read out rather than open up, on both sides of the split
        // above.
        let dates = match (card.meta.created.trim(), card.meta.updated.trim()) {
            ("", "") => None,
            ("", updated) => Some(format!("Updated {updated}")),
            (created, "") => Some(format!("Created {created}")),
            (created, updated) => Some(format!("Created {created}, updated {updated}")),
        };
        body.children(dates.map(|dates| muted(dates.into())))
            .into_any_element()
    }

    /// Open a workspace's card, or close it when it's the one already open.
    /// Opening reads the bundle once and seeds the inputs from it, so the
    /// fields show what's in the file rather than what was there the last
    /// time this window looked.
    fn toggle_workspace_card(
        &mut self,
        name: &str,
        builtin: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspace_card
            .as_ref()
            .is_some_and(|card| card.name == name)
        {
            self.workspace_card = None;
            cx.notify();
            return;
        }
        let Some(bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        let meta = bundle.meta.clone();
        let fields = (!builtin).then(|| {
            CARD_FIELDS
                .iter()
                .map(|(_, placeholder, field)| {
                    let mut seed = meta.clone();
                    let value = field(&mut seed).clone();
                    cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(*placeholder)
                            .default_value(value)
                    })
                })
                .collect()
        });
        self.workspace_card = Some(CardEditor {
            name: name.to_string(),
            meta,
            fields,
        });
        cx.notify();
    }

    /// Write the open card back into its workspace file. Reads the bundle
    /// fresh and replaces only its card, so a save here never touches the
    /// layouts, palette, or shaders sitting beside it.
    ///
    /// Resolving by name rather than reading the file directly is deliberate:
    /// `read_bundle` is the import path and dedupes against the workspaces
    /// already saved, which for a workspace that is one of them would rename
    /// it out from under the edit.
    fn save_workspace_card(&mut self, cx: &mut Context<Self>) {
        let Some(card) = self.workspace_card.as_ref() else {
            return;
        };
        if card.fields.is_none() {
            return;
        }
        let Some(mut bundle) = crate::workspaces::resolve(&card.name) else {
            return;
        };
        bundle.meta = card.typed(cx);
        // The list names a saved workspace after its file, so the write goes
        // back under that name: a hand-dropped file whose bundle says
        // something else would otherwise save to a second file beside it.
        bundle.name = card.name.clone();
        crate::workspaces::store(&bundle);
        if let Some(card) = self.workspace_card.as_mut() {
            card.meta = bundle.meta;
        }
        self.workspace_authors = crate::workspaces::saved_authors();
        cx.notify();
    }

    /// The presets section: the saved and shipped layouts as a list, each
    /// with the roles the mini-player button toggles between and the ways
    /// to apply, delete, or overwrite it. Saving the live layout as a named
    /// preset rides the header.
    fn presets_section(&self, q: &Query, live: bool, cx: &mut Context<Self>) -> Section {
        let settings = Settings::load();
        let presets = rox_core::settings::layouts::all(&settings);

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

        Section::new(
            q,
            icons::LAYOUT_GRID,
            "Layouts",
            Some(save.into_any_element()),
            |rows| {
                rows.custom(
                    &["preset", "dock", "panels", "mini", "primary", "save"],
                    || {
                        let mut list =
                            div()
                                .flex()
                                .flex_col()
                                .gap(tokens::SPACE_XS)
                                .child(div().text_xs().text_color(palette::text_muted()).child(
                                "Primary and mini are the two the menubar's mini-player button \
                             swaps between",
                            ));
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
                        list.into_any_element()
                    },
                )
            },
        )
    }

    /// The panel presets section: the saved single panels as a list. They are
    /// made and replaced from the panel they hold (its dropdown's Save As
    /// Preset), so this list is where you see what the look carries and drop
    /// what you're done with.
    fn panel_presets_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let presets = crate::panel_presets::saved();

        Section::new(q, icons::COPY, "Panel Presets", None, |rows| {
            rows.custom(
                &["panel", "preset", "saved", "configured", "add panel"],
                || {
                    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                        // Same instruction the save dialog gives, so it wears
                        // the same keycaps for the menu path.
                        kbd_line([
                            Seg::Text(
                                "One configured panel each, saved from a panel's own menu and \
                                 added back from"
                                    .into(),
                            ),
                            Seg::Key("Add Panel".into()),
                            Seg::Text("then".into()),
                            Seg::Key("Presets".into()),
                            Seg::Text(
                                "in any panel menu. They ride this workspace only, so another \
                                 workspace won't carry them."
                                    .into(),
                            ),
                        ])
                        .text_xs(),
                    );
                    if presets.is_empty() {
                        list = list.child(
                            div()
                                .text_color(palette::text_muted())
                                .child("No panel presets yet"),
                        );
                    } else {
                        list = list.child(
                            div().flex().flex_col().children(
                                presets
                                    .into_iter()
                                    .map(|preset| self.panel_preset_row(preset, cx)),
                            ),
                        );
                    }
                    list.into_any_element()
                },
            )
        })
    }

    /// One panel preset's row: its name, the kind of panel inside it, and the
    /// delete. The kind is what tells two presets of the same panel apart
    /// from two of different ones once the names blur.
    fn panel_preset_row(
        &self,
        preset: rox_core::settings::PanelPreset,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = preset
            .panel_name()
            .map(rox_panel_api::panel::display_name)
            .unwrap_or_else(|| "Unknown panel".into());
        let icon = crate::panel_presets::icon_for(&preset);
        let name = preset.name;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .child(
                svg()
                    .path(icon)
                    .size_3p5()
                    .text_color(palette::text_muted()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(name.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(kind)),
            )
            .child(icon_button(icons::TRASH, false, {
                cx.listener(move |_, _, _, cx| {
                    rox_core::settings::panel_presets::remove(&name);
                    cx.notify();
                })
            }))
            .into_any_element()
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
        if Settings::load()
            .look
            .bundle
            .layouts
            .iter()
            .any(|l| l.name == name)
        {
            self.pending = Some(Pending::OverwritePreset(name));
            cx.notify();
            return;
        }
        let dump = workspace.read(cx).dock().read(cx).dump(cx);
        let Ok(dump) = serde_json::to_value(dump) else {
            return;
        };
        let size = self.workspace_window_size(cx);
        Settings::update(move |s| s.look.bundle.layouts.push(NamedLayout { name, dump, size }));
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
                    if let Some(existing) =
                        s.look.bundle.layouts.iter_mut().find(|l| l.name == name)
                    {
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
        Settings::update(move |s| s.look.bundle.primary_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    /// Point the mini role at a preset, or clear it when the preset already
    /// holds it.
    fn set_mini(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.mini_layout.as_deref() == Some(name);
        self.mini_layout = (!clear).then(|| name.to_string());
        let value = self.mini_layout.clone();
        Settings::update(move |s| s.look.bundle.mini_layout = value);
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
            s.look.layout_edits.remove(name.as_str());
            s.look.bundle.layouts.retain(|l| l.name != name);
            if s.look.bundle.primary_layout.as_deref() == Some(name.as_str()) {
                s.look.bundle.primary_layout = None;
            }
            if s.look.bundle.mini_layout.as_deref() == Some(name.as_str()) {
                s.look.bundle.mini_layout = None;
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
        // A workspace apply reads out what's coming before it lands: who made
        // it, what they say it is, and any shader code riding along that this
        // machine has never agreed to run.
        let card = match self.pending.as_ref()? {
            Pending::ApplyWorkspace { card, .. } => Some(card),
            _ => None,
        };
        let shaders = card.and_then(|card| card.shader_line());
        let screen = card.and_then(|card| card.screen_shader.clone());
        // Whether the yes splits in two. Code nobody has agreed to splits it,
        // and so does a look that simply wears shaders, however many times
        // it's been applied before.
        let split = card.is_some_and(|card| card.splits_apply());
        let (title, body, confirm): (String, SharedString, &'static str) =
            match self.pending.as_ref()? {
                Pending::OverwritePreset(name) => (
                    format!("Overwrite \"{name}\"?"),
                    "This replaces the saved layout with the current one.".into(),
                    "Overwrite",
                ),
                Pending::OverwriteWorkspace(name) => (
                    format!("Overwrite workspace \"{name}\"?"),
                    "This replaces the saved workspace with the current state.".into(),
                    "Overwrite",
                ),
                Pending::ApplyWorkspace {
                    card,
                    imported: true,
                } => (
                    format!("Imported \"{}\"", card.name),
                    "It's saved to your workspaces. Applying it now replaces your layouts, \
                     palette, and appearance with the workspace's."
                        .into(),
                    "Apply",
                ),
                Pending::ApplyWorkspace { card, .. } => (
                    format!("Apply \"{}\"?", card.name),
                    "This replaces your layouts, palette, and appearance with the workspace's."
                        .into(),
                    "Apply",
                ),
            };
        let line = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
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
                        // The shader list and the screen shader's hotkey line
                        // both need the room; every other confirm keeps the
                        // dialogs' shared width.
                        .w(px(if split || screen.is_some() {
                            380.
                        } else {
                            320.
                        }))
                        .p(tokens::SPACE_MD)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_menu_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(div().child(SharedString::from(title)))
                        .children(card.and_then(|card| card.byline.clone()).map(line))
                        .children(card.and_then(|card| card.description.clone()).map(line))
                        .child(line(body))
                        // A screen shader covers the whole window, so it gets
                        // said before the apply rather than asked about after,
                        // and the way back off comes with it.
                        .children(screen.clone().map(line))
                        .children(screen.map(|_| {
                            kbd_line([
                                Seg::Text("Turn it off any time with".into()),
                                Seg::Key(chord("Shift+X")),
                                Seg::Text("or".into()),
                                Seg::Key("Window".into()),
                                Seg::Text("then".into()),
                                Seg::Key("Overlay Shader".into()),
                            ])
                            .text_xs()
                        }))
                        .children(shaders.clone().map(line))
                        // Shaders that came with a look are somebody else's
                        // code, so the yes that runs them says so, and the yes
                        // that doesn't is right beside it. Once they're agreed
                        // to the question is only about the look, and the line
                        // says that instead.
                        .children(split.then(|| {
                            line(if shaders.is_some() {
                                "Approving lets them run on this machine. Applying without \
                                 them leaves the look bare, with the shaders still in its pool."
                                    .into()
                            } else {
                                SharedString::from(
                                    "Applying without them leaves the look bare, with the \
                                     shaders still in its pool.",
                                )
                            })
                        }))
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
                                    if split { "Without Shaders" } else { confirm },
                                    !split,
                                    cx.listener(|this, _, window, cx| {
                                        this.confirm_pending(ApplyShaders::Skip, window, cx)
                                    }),
                                ))
                                .children(split.then(|| {
                                    dialog_button(
                                        if shaders.is_some() {
                                            "Approve and Apply"
                                        } else {
                                            "With Shaders"
                                        },
                                        true,
                                        cx.listener(|this, _, window, cx| {
                                            this.confirm_pending(ApplyShaders::Wear, window, cx)
                                        }),
                                    )
                                })),
                        ),
                ),
        )
    }

    /// Carry out the pending action, the confirm dialog's yes, and clear it.
    /// `shaders` separates the apply dialog's two yes buttons: the wearing one
    /// agrees to the shaders the bundle brought, and it's the only thing on
    /// this path that ever writes the approved list.
    fn confirm_pending(
        &mut self,
        shaders: ApplyShaders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.pending.take() {
            Some(Pending::OverwritePreset(name)) => self.overwrite_preset(name, window, cx),
            Some(Pending::OverwriteWorkspace(name)) => self.overwrite_workspace(name, window, cx),
            Some(Pending::ApplyWorkspace { card, .. }) => {
                if shaders == ApplyShaders::Wear {
                    card.approve_shaders();
                }
                self.apply_workspace(&card.name, shaders, window, cx);
            }
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

    /// A panel's row, and under a composite host (group, overlay, drawer,
    /// slide) its hosted children as indented rows of their own, so the
    /// tree shows what the host holds instead of one opaque line.
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
                    // Recurse: a host can hold another host (a drawer
                    // inside a drawer), and the tree should keep going
                    // down instead of stopping at the inner line.
                    Some(child) => self.panel_rows(child, depth + 1, TreeSlot::Hosted, rows, cx),
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
                        // A closed lock is state worth seeing at rest;
                        // the open one only shows with the row's other
                        // controls.
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
        let Some(preset) = rox_core::settings::layouts::resolve(&Settings::load(), name) else {
            return;
        };
        // Denoise on the way out too, not just at save: a preset saved before
        // the store-time pass still carries widened f64 tails in settings.
        let mut dump = preset.dump;
        crate::workspace::denoise_f32(&mut dump);
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
                rox_core::settings::layouts::all(&Settings::load())
                    .iter()
                    .any(|p| p.name == candidate)
            });
            Settings::update(move |s| {
                s.look.bundle.layouts.push(NamedLayout {
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
        if crate::workspaces::path_for(&name).exists() {
            self.pending = Some(Pending::OverwriteWorkspace(name));
            cx.notify();
            return;
        }
        crate::workspaces::store(&crate::workspaces::snapshot(&name, &Settings::load()));
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.workspace_authors = crate::workspaces::saved_authors();
        cx.notify();
    }

    /// Replace a saved workspace with the current state, the confirm dialog's
    /// yes. Clears the name field.
    fn overwrite_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_workspace_layout(cx);
        // The bundle's name picks its file, so the overwrite lands back on the
        // one the first save wrote, and the snapshot carries the card that
        // file already had rather than blanking it.
        crate::workspaces::store(&crate::workspaces::snapshot(&name, &Settings::load()));
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.workspace_authors = crate::workspaces::saved_authors();
        // A card open on the workspace that just got replaced came out of the
        // old file; re-read it so the fields show what the overwrite wrote.
        // Any other workspace's card is untouched by this write.
        let reopen = self
            .workspace_card
            .as_ref()
            .filter(|card| card.name == name)
            .map(|card| card.name.clone());
        if let Some(name) = reopen {
            self.workspace_card = None;
            self.toggle_workspace_card(&name, false, window, cx);
        }
        cx.notify();
    }

    /// Delete a user workspace. Shipped ones carry no delete.
    fn delete_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        crate::workspaces::remove(name);
        if self
            .workspace_card
            .as_ref()
            .is_some_and(|card| card.name == name)
        {
            self.workspace_card = None;
        }
        self.workspace_authors = crate::workspaces::saved_authors();
        cx.notify();
    }

    /// Export a workspace bundle to a file, the whole look as one shareable
    /// artifact. Works for shipped bundles too.
    ///
    /// Shader assets travel inside the file as encoded bytes, so a look that
    /// stamps plates weighs what its images weigh. Past [`EXPORT_SIZE_WARN`]
    /// that's worth saying out loud, and no more than that: a legitimate look
    /// can be big, and a hard cap would only stop one (ADR 23). The note goes
    /// to the log, which is where this window's writes report themselves, and
    /// the export happens regardless.
    fn export_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mut bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        // Same denoise as the preset export: clean any widened f64 tails in the
        // bundled layout dumps. Done in place so the bundle's own field order
        // survives (routing it through serde_json::Value would sort the keys).
        for layout in &mut bundle.layouts {
            crate::workspace::denoise_f32(&mut layout.dump);
        }
        let home = dirs::home_dir().unwrap_or_default();
        let file = format!("{name}.json");
        let label = name.to_string();
        let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&bundle) {
                if json.len() > EXPORT_SIZE_WARN {
                    log::warn!(
                        "workspace {label:?}: exported at {:.1} MiB, heavier than a look usually runs. Its shader assets ride inside the file.",
                        json.len() as f64 / (1024.0 * 1024.0)
                    );
                }
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    /// Pick a workspace file and add it to the collection, named after the
    /// file when the bundle carries no name of its own and deduped so an
    /// import never shadows an existing workspace. A bundle from a newer
    /// format, or a file that isn't a bundle, is ignored.
    ///
    /// A bundle carrying shaders this machine has never agreed to run opens
    /// the apply confirm on the way in, so what arrived gets read out at the
    /// moment it lands rather than a week later when somebody applies it.
    /// Backing out of that dialog is exactly the old behaviour: the file is
    /// saved, nothing is approved, and nothing is wearing it.
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
            let Some(bundle) = crate::workspaces::read_bundle(&path) else {
                return;
            };
            crate::workspaces::store(&bundle);
            let card = crate::workspaces::ApplyCard::of(&bundle);
            this.update(cx, |this, cx| {
                if !card.shaders.is_empty() {
                    this.pending = Some(Pending::ApplyWorkspace {
                        card,
                        imported: true,
                    });
                }
                this.workspace_authors = crate::workspaces::saved_authors();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Apply a workspace: replace the live look wholesale, through the
    /// workspace's own apply so the persist, the active-layout guard, and
    /// the no-layout fallback to the default arrangement all ride one flow.
    /// This window only mirrors the applied look into its own editor state
    /// on top.
    fn apply_workspace(
        &mut self,
        name: &str,
        shaders: ApplyShaders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        // The workspace's own apply strips its copy the same way; this one is
        // for the no-dock fallback below and for the mirror that follows it.
        let bundle = match shaders {
            ApplyShaders::Wear => bundle,
            ApplyShaders::Skip => crate::workspaces::without_shaders(&bundle),
        };
        let workspace = self.workspace.clone();
        let name = name.to_string();
        let applied = self
            .workspace_window
            .update(cx, |_, window, cx| {
                workspace.upgrade().is_some_and(|workspace| {
                    workspace.update(cx, |workspace, cx| {
                        workspace.apply_workspace(
                            &name,
                            shaders,
                            crate::workspace::ShaderNotice::Told,
                            window,
                            cx,
                        );
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
        // idempotent. The apply may have flipped the theme side, so the
        // editor re-seeds onto whichever side now renders.
        self.editor_mode = palette::mode();
        let mirrored = match self.editor_mode {
            palette::Mode::Dark => Palette::from_map(&bundle.palette_dark),
            palette::Mode::Light => Palette::from_map_over(Palette::light(), &bundle.palette_light),
        };
        self.apply_palette(mirrored, window, cx);
        let a = &bundle.appearance;
        self.surface_opacity = a.surface_opacity;
        self.backdrop_strength = a.backdrop_strength;
        self.frame = a.frame;
        self.keep_theme = a.keep_theme;
        self.rating_style = a.rating_style;
        // The mini-player roles; the workspace's apply already moved its own
        // live copy along with the dock.
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        cx.notify();
    }
}

/// One line of a shipped bundle's card, read out rather than typed in: the
/// label in a column of its own, the value wrapping in what's left. The
/// editable side uses `setting_row`'s inline control instead, since an input
/// is one line high whatever's in it, while a description comes out of the
/// file however long its author wrote it.
fn card_readout_line(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(tokens::SPACE_MD)
        .text_xs()
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(SharedString::from(value)))
}
