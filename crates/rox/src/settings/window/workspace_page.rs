//! The Workspace settings page: workspaces, layout presets and panel presets,
//! the composition tree of the opening window's dock, and the confirm dialog
//! every destructive action in the window goes through.

use super::*;

/// Built when a row's details open and dropped when they close.
pub(crate) struct CardEditor {
    name: String,
    /// The card as the bundle arrived; the whole readout for a shipped bundle.
    meta: WorkspaceMeta,
    /// None for a shipped bundle, whose file lives in the app's assets.
    fields: Option<Vec<Entity<InputState>>>,
}

/// Dates stay out: a save stamps them. The label and placeholder are i18n keys,
/// since a `const` table can't call the translator.
type CardField = (
    &'static str,
    &'static str,
    fn(&mut WorkspaceMeta) -> &mut String,
);
const CARD_FIELDS: [CardField; 5] = [
    (
        "settings-workspace-card-author",
        "settings-workspace-card-author-placeholder",
        |meta| &mut meta.author,
    ),
    (
        "settings-workspace-card-description",
        "settings-workspace-card-description-placeholder",
        |meta| &mut meta.description,
    ),
    (
        "settings-workspace-card-website",
        "settings-workspace-card-website-placeholder",
        |meta| &mut meta.website,
    ),
    (
        "settings-workspace-card-version",
        "settings-workspace-card-version-placeholder",
        |meta| &mut meta.version,
    ),
    (
        "settings-workspace-card-license",
        "settings-workspace-card-license-placeholder",
        |meta| &mut meta.license,
    ),
];

/// Soft: past this an export logs a note, never a refusal. Image assets push a
/// look into megabytes.
const EXPORT_SIZE_WARN: usize = 4 * 1024 * 1024;

/// Most dialogs only see `First`; the two that split read `Second` as the wider
/// answer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Yes {
    First,
    Second,
}

fn apply_yes(split: bool) -> SharedString {
    if split {
        rox_i18n::t!("workspace-dialog-without-shaders")
    } else {
        rox_i18n::t!("workspace-dialog-apply")
    }
}

fn apply_second_yes(split: bool, unapproved: bool) -> Option<SharedString> {
    split.then(|| {
        if unapproved {
            rox_i18n::t!("workspace-dialog-approve-apply")
        } else {
            rox_i18n::t!("workspace-dialog-with-shaders")
        }
    })
}

impl CardEditor {
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
    pub(crate) fn workspace_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let live = self.workspace.upgrade().is_some();
        PageBody::new()
            .section(self.workspaces_section(q, live, cx))
            .section(self.presets_section(q, live, cx))
            .section(self.panel_presets_section(q, cx))
            // Built only once the query keeps it, since it walks the live dock.
            .section(Section::new(
                q,
                icons::LAYOUT_DASHBOARD,
                rox_i18n::t!("settings-workspace-section-composition"),
                None,
                |rows| {
                    rows.custom(
                        &["dock", "panels", "tree", "splits", "tabs", "layout"],
                        || {
                            let mut body = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-workspace-composition-hint")),
                            );
                            match self.workspace.upgrade() {
                                Some(workspace) => {
                                    let root = workspace.read(cx).dock().read(cx).items().view();
                                    let mut rows = Vec::new();
                                    self.tree_rows(root, 0, TreeSlot::Root, &mut rows, cx);
                                    body = body.child(div().flex().flex_col().children(rows));
                                }
                                None => {
                                    body =
                                        body.child(div().text_color(palette::text_muted()).child(
                                            rox_i18n::t!("settings-workspace-composition-closed"),
                                        ));
                                }
                            }
                            body.into_any_element()
                        },
                    )
                },
            ))
    }

    fn workspaces_section(&self, q: &Query, live: bool, cx: &mut Context<Self>) -> Section {
        let entries = crate::workspaces::all();

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.workspace_name).small().w(px(150.)))
            .child(small_button(
                rox_i18n::t!("workspace-save-current"),
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.save_workspace(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("workspace-import"),
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_workspace(window, cx)),
            ));

        Section::new(
            q,
            icons::APP_WINDOW,
            rox_i18n::t!("settings-workspace-section-workspaces"),
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(
                    &["look", "bundle", "theme", "import", "export", "apply"],
                    || {
                        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("settings-workspace-hint")),
                        );
                        if entries.is_empty() {
                            list = list.child(
                                div()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-workspace-empty")),
                            );
                        } else {
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

    fn workspace_row(
        &self,
        entry: crate::workspaces::Entry,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = entry.name.clone();
        let title = entry.title.clone();
        let author = entry
            .author
            .clone()
            .or_else(|| self.workspace_authors.get(&name).cloned());
        let open = self
            .workspace_card
            .as_ref()
            .is_some_and(|card| card.name == name);
        div()
            // Named after the workspace: every row says Apply and Export, and
            // ids nest. See `rox_panel_kit::ui::control_focus`.
            .id(ElementId::Name(format!("workspace-row:{name}").into()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            // The chevron leads so it points at the name it expands, apart from
            // the action buttons.
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
                    .child(div().truncate().child(title.clone()))
                    .when_some(author, |d, author| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("workspace-byline-author", author = author)),
                        )
                    }),
            )
            .when(entry.builtin, |d| d.child(shipped_tag()))
            .child(small_button(
                rox_i18n::t!("workspace-dialog-apply"),
                icons::CHECK,
                !live,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.pending = Some(Pending::ApplyWorkspace {
                            card: crate::workspaces::ApplyCard::for_name(&name),
                            imported: false,
                        });
                        cx.notify();
                    })
                },
            ))
            .when(!entry.builtin, |d| {
                // A shipped workspace has no export, overwrite or delete.
                // Overwrite confirms first; delete doesn't.
                d.child(small_button(
                    rox_i18n::t!("workspace-dialog-export"),
                    icons::UPLOAD,
                    false,
                    {
                        let name = name.clone();
                        cx.listener(move |this, _, _, cx| this.export_workspace(&name, cx))
                    },
                ))
                .child(small_button(
                    rox_i18n::t!("workspace-dialog-overwrite"),
                    icons::REFRESH_CW,
                    !live,
                    {
                        let name = name.clone();
                        cx.listener(move |this, _, _, cx| {
                            this.pending = Some(Pending::OverwriteWorkspace(name.clone()));
                            cx.notify();
                        })
                    },
                ))
                .child(icon_button(icons::TRASH, false, {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.delete_workspace(&name, cx))
                }))
            })
            .into_any_element()
    }

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
        // Indented past the chevron to where the row's name starts.
        let mut body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .pb(tokens::SPACE_SM)
            .pl(px(14.) + tokens::SPACE_XS * 2. + tokens::SPACE_SM);
        match card.fields.as_ref() {
            Some(fields) => {
                body = body.child(muted(rox_i18n::t!("settings-workspace-card-hint")));
                for ((label, _, _), input) in CARD_FIELDS.iter().zip(fields) {
                    body = body.child(panel::setting_row(
                        rox_i18n::t!(*label),
                        None,
                        Input::new(input).small().w(px(240.)),
                    ));
                }
                body = body.child(div().flex().flex_row().justify_end().child(small_button(
                    rox_i18n::t!("settings-workspace-card-save"),
                    icons::CHECK,
                    false,
                    cx.listener(|this, _, _, cx| this.save_workspace_card(cx)),
                )));
            }
            // Fork a shipped bundle with Save Current to get a card of your
            // own.
            None if card.meta.is_empty() => {
                body = body.child(muted(rox_i18n::t!("settings-workspace-card-empty")));
            }
            None => {
                // A shipped look's blurb is rox's own prose, so it resolves
                // through the locales like the apply dialog does.
                let mut meta = card.meta.clone();
                if let Some(blurb) = crate::workspaces::display_blurb(&card.name, &meta.description)
                {
                    meta.description = blurb.to_string();
                }
                for (label, _, field) in CARD_FIELDS {
                    let value = field(&mut meta).clone();
                    if value.trim().is_empty() {
                        continue;
                    }
                    body = body.child(card_readout_line(rox_i18n::t!(label), value));
                }
            }
        }
        // Read out on both sides: a save stamps the dates.
        let dates = match (card.meta.created.trim(), card.meta.updated.trim()) {
            ("", "") => None,
            ("", updated) => Some(rox_i18n::t!(
                "settings-workspace-card-updated",
                date = rox_i18n::format::format_iso_date(updated)
            )),
            (created, "") => Some(rox_i18n::t!(
                "settings-workspace-card-created",
                date = rox_i18n::format::format_iso_date(created)
            )),
            (created, updated) => Some(rox_i18n::t!(
                "settings-workspace-card-created-updated",
                created = rox_i18n::format::format_iso_date(created),
                updated = rox_i18n::format::format_iso_date(updated)
            )),
        };
        body.children(dates.map(muted)).into_any_element()
    }

    /// Reads the bundle once on open, so the fields show the file, not a stale
    /// copy.
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
                            .placeholder(rox_i18n::t!(*placeholder))
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

    /// Replaces only the card. Resolved by name: `read_bundle` is the import
    /// path and dedupes, which would rename the workspace out from under the
    /// edit.
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
        // Saved under the file's name, or a hand-dropped file whose bundle says
        // otherwise would get a second file.
        bundle.name = card.name.clone();
        crate::workspaces::store(&bundle);
        if let Some(card) = self.workspace_card.as_mut() {
            card.meta = bundle.meta;
        }
        self.workspace_authors = crate::workspaces::saved_authors();
        cx.notify();
    }

    fn presets_section(&self, q: &Query, live: bool, cx: &mut Context<Self>) -> Section {
        let settings = Settings::load();
        let presets = rox_core::settings::layouts::all(&settings);

        let save = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.layout_name).small().w(px(150.)))
            .child(small_button(
                rox_i18n::t!("workspace-save-current"),
                icons::DOWNLOAD,
                !live,
                cx.listener(|this, _, window, cx| this.save_layout_preset(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("workspace-import"),
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_preset(window, cx)),
            ));

        Section::new(
            q,
            icons::LAYOUT_GRID,
            rox_i18n::t!("settings-workspace-section-layouts"),
            Some(save.into_any_element()),
            |rows| {
                rows.custom(
                    &["preset", "dock", "panels", "mini", "primary", "save"],
                    || {
                        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("settings-workspace-layouts-hint")),
                        );
                        if presets.is_empty() {
                            list = list.child(
                                div()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-workspace-layouts-empty")),
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

    fn panel_presets_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let presets = crate::panel_presets::saved();

        Section::new(
            q,
            icons::COPY,
            rox_i18n::t!("settings-workspace-section-panel-presets"),
            None,
            |rows| {
                rows.custom(
                    &["panel", "preset", "saved", "configured", "add panel"],
                    || {
                        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS).child(
                            // The save dialog's instruction, with the same keycaps.
                            kbd_line([
                                Seg::Text(rox_i18n::t!(
                                    "settings-workspace-panel-presets-hint-before"
                                )),
                                Seg::Key(rox_i18n::t!("workspace-context-add-panel")),
                                Seg::Text(rox_i18n::t!("workspace-hint-then")),
                                Seg::Key(rox_i18n::t!("menu-panels-presets")),
                                Seg::Text(rox_i18n::t!(
                                    "settings-workspace-panel-presets-hint-after"
                                )),
                            ])
                            .text_xs(),
                        );
                        if presets.is_empty() {
                            list = list.child(
                                div()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-workspace-panel-presets-empty")),
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
            },
        )
    }

    fn panel_preset_row(
        &self,
        preset: rox_core::settings::PanelPreset,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = preset
            .panel_name()
            .map(rox_panel_api::panel::display_name)
            .unwrap_or_else(|| {
                rox_i18n::t!("settings-workspace-panel-preset-unknown-kind").to_string()
            });
        let icon = crate::panel_presets::icon_for(&preset);
        let name = preset.name;
        div()
            .id(ElementId::Name(format!("panel-preset-row:{name}").into()))
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

    fn preset_row(&self, preset: Preset, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let is_primary = self.primary_layout.as_deref() == Some(preset.name.as_str());
        let is_mini = self.mini_layout.as_deref() == Some(preset.name.as_str());
        let name = preset.name.clone();
        div()
            .id(ElementId::Name(format!("preset-row:{name}").into()))
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
            .child(role_chip(
                rox_i18n::t_static("settings-workspace-role-primary"),
                is_primary,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.set_primary(&name, cx))
                },
            ))
            .child(role_chip(
                rox_i18n::t_static("settings-workspace-role-mini"),
                is_mini,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.set_mini(&name, cx))
                },
            ))
            .child(small_button(
                rox_i18n::t!("workspace-dialog-apply"),
                icons::CHECK,
                !live,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.apply_preset(&name, cx))
                },
            ))
            .child(small_button(
                rox_i18n::t!("workspace-dialog-export"),
                icons::UPLOAD,
                false,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| this.export_preset(&name, cx))
                },
            ))
            // Overwrite confirms first; apply and delete don't.
            .child(small_button(
                rox_i18n::t!("workspace-dialog-overwrite"),
                icons::REFRESH_CW,
                !live,
                {
                    let name = name.clone();
                    cx.listener(move |this, _, _, cx| {
                        this.pending = Some(Pending::OverwritePreset(name.clone()));
                        cx.notify();
                    })
                },
            ))
            .child(icon_button(icons::TRASH, false, {
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.delete_preset(&name, cx))
            }))
            .into_any_element()
    }

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

    fn set_primary(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.primary_layout.as_deref() == Some(name);
        self.primary_layout = (!clear).then(|| name.to_string());
        let value = self.primary_layout.clone();
        Settings::update(move |s| s.look.bundle.primary_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

    fn set_mini(&mut self, name: &str, cx: &mut Context<Self>) {
        let clear = self.mini_layout.as_deref() == Some(name);
        self.mini_layout = (!clear).then(|| name.to_string());
        let value = self.mini_layout.clone();
        Settings::update(move |s| s.look.bundle.mini_layout = value);
        self.sync_roles_to_workspace(cx);
        cx.notify();
    }

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

    /// A split yes also takes Enter away: a question gets a click.
    fn splits_yes(&self, pending: &Pending) -> bool {
        match pending {
            // Unapproved code splits an apply, and so does any look with
            // shaders.
            Pending::ApplyWorkspace { card, .. } => card.splits_apply(),
            // Only with imported rows to tell apart; otherwise "none" and "all"
            // are one answer.
            Pending::ClearListens => self.listens().imported > 0,
            _ => false,
        }
    }

    /// Read off the storage walk: every path here starts from a row that walk
    /// drew.
    fn listens(&self) -> rox_library::listens::Tally {
        self.storage.as_ref().map(|s| s.listens).unwrap_or_default()
    }

    /// Escape backs out; Enter takes the yes except where it splits.
    fn confirm_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        if event.keystroke.modifiers.modified() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.pending = None;
                cx.notify();
                true
            }
            // Only while the dialog itself holds focus: Enter on a focused
            // Cancel must not mean yes.
            "enter" if self.dialog_focus.is_focused(window) => {
                if self.splits_yes(pending) {
                    return false;
                }
                self.confirm_pending(Yes::First, window, cx);
                true
            }
            _ => false,
        }
    }

    /// A scrim occludes the page; only the buttons, Enter and Escape close it,
    /// no click-away.
    pub(crate) fn confirm_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        // An apply reads out who made the look, what it is, and any unapproved
        // shader code.
        let card = match self.pending.as_ref()? {
            Pending::ApplyWorkspace { card, .. } => Some(card),
            _ => None,
        };
        let shaders = card.and_then(|card| card.shader_line());
        let screen = card.and_then(|card| card.screen_shader.clone());
        let split = self.splits_yes(self.pending.as_ref()?);
        let listens = self.listens();
        let (title, body, confirm, second): (
            SharedString,
            SharedString,
            SharedString,
            Option<SharedString>,
        ) = match self.pending.as_ref()? {
            Pending::OverwritePreset(name) => (
                rox_i18n::t!("workspace-dialog-overwrite-title", name = name.as_str()),
                rox_i18n::t!("workspace-layout-overwrite-body"),
                rox_i18n::t!("workspace-dialog-overwrite"),
                None,
            ),
            Pending::OverwriteWorkspace(name) => (
                rox_i18n::t!(
                    "settings-confirm-overwrite-workspace-title",
                    name = name.as_str()
                ),
                rox_i18n::t!("settings-confirm-overwrite-workspace-body"),
                rox_i18n::t!("workspace-dialog-overwrite"),
                None,
            ),
            Pending::ApplyWorkspace {
                card,
                imported: true,
            } => (
                rox_i18n::t!("workspace-apply-imported-title", name = card.name.as_str()),
                rox_i18n::t!("settings-confirm-apply-imported-body"),
                apply_yes(split),
                apply_second_yes(split, shaders.is_some()),
            ),
            Pending::ApplyWorkspace { card, .. } => (
                rox_i18n::t!("workspace-dialog-apply-title", name = card.name.as_str()),
                rox_i18n::t!("settings-confirm-apply-body"),
                apply_yes(split),
                apply_second_yes(split, shaders.is_some()),
            ),
            Pending::ClearEmbeddings(model) => (
                rox_i18n::t!(
                    "settings-confirm-clear-embeddings-title",
                    model = model.as_str()
                ),
                rox_i18n::t!("settings-confirm-clear-embeddings-body"),
                rox_i18n::t!("settings-confirm-clear"),
                None,
            ),
            Pending::ClearMeasuredBpm => (
                rox_i18n::t!("settings-confirm-clear-measured-bpm-title"),
                rox_i18n::t!("settings-confirm-clear-measured-bpm-body"),
                rox_i18n::t!("settings-confirm-clear"),
                None,
            ),
            Pending::ClearListens if split => (
                rox_i18n::t!("listens-clear-title"),
                rox_i18n::t!(
                    "listens-clear-body",
                    imported = listens.imported,
                    total = listens.total
                ),
                rox_i18n::t!("listens-clear-imported"),
                Some(rox_i18n::t!("listens-clear-everything")),
            ),
            Pending::RemoveSubsonic(id) => (
                rox_i18n::t!(
                    "settings-confirm-remove-subsonic-title",
                    name = self.subsonic_label(*id, cx).to_string()
                ),
                rox_i18n::t!("settings-confirm-remove-subsonic-body"),
                rox_i18n::t!("settings-common-remove"),
                None,
            ),
            Pending::ClearListens => (
                rox_i18n::t!("listens-clear-title"),
                rox_i18n::t!(
                    "listens-clear-body-plain",
                    listens = rox_i18n::t!("listens-count", count = listens.total).to_string()
                ),
                rox_i18n::t!("settings-confirm-clear"),
                None,
            ),
        };
        let line = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
        };
        // Take focus unless it's already inside, or Tab through the buttons
        // would snap back each frame.
        if !self.dialog_focus.contains_focused(window, cx) {
            window.focus(&self.dialog_focus);
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .track_focus(&self.dialog_focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if this.confirm_key(event, window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        // The shader list and the hotkey line need the room.
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
                        .child(div().child(title))
                        .children(card.and_then(|card| card.byline.clone()).map(line))
                        .children(card.and_then(|card| card.description.clone()).map(line))
                        .child(line(body))
                        // A screen shader covers the window, so say so before the
                        // apply, with the way back off.
                        .children(screen.clone().map(line))
                        .children(screen.map(|_| {
                            kbd_line([
                                Seg::Text(rox_i18n::t!("workspace-screen-shader-hint-before")),
                                Seg::Key(chord("Shift+X")),
                                Seg::Text(rox_i18n::t!("workspace-hint-or")),
                                Seg::Key(rox_i18n::t!("menu-window")),
                                Seg::Text(rox_i18n::t!("workspace-hint-then")),
                                Seg::Key(rox_i18n::t!("menu-overlay-shader")),
                            ])
                            .text_xs()
                        }))
                        .children(shaders.clone().map(line))
                        // Shaders from a look are somebody else's code, so the
                        // yes that runs them says so.
                        .children(split.then(|| {
                            line(if shaders.is_some() {
                                rox_i18n::t!("workspace-apply-shaders-approve-body")
                            } else {
                                rox_i18n::t!("workspace-apply-shaders-plain-body")
                            })
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap(tokens::SPACE_SM)
                                .child(dialog_button(
                                    rox_i18n::t!("workspace-dialog-cancel"),
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.pending = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(dialog_button(
                                    confirm,
                                    !split,
                                    cx.listener(|this, _, window, cx| {
                                        this.confirm_pending(Yes::First, window, cx)
                                    }),
                                ))
                                .children(second.map(|label| {
                                    dialog_button(
                                        label,
                                        true,
                                        cx.listener(|this, _, window, cx| {
                                            this.confirm_pending(Yes::Second, window, cx)
                                        }),
                                    )
                                })),
                        ),
                ),
        )
    }

    /// On an apply, `Yes::Second` approves the bundle's shaders, the only write
    /// to the approved list on this path. On a listens clear it widens the
    /// delete to the whole record.
    fn confirm_pending(&mut self, yes: Yes, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.take() {
            Some(Pending::OverwritePreset(name)) => self.overwrite_preset(name, window, cx),
            Some(Pending::OverwriteWorkspace(name)) => self.overwrite_workspace(name, window, cx),
            Some(Pending::ApplyWorkspace { card, .. }) => {
                let shaders = match yes {
                    Yes::Second => ApplyShaders::Wear,
                    Yes::First => ApplyShaders::Skip,
                };
                if shaders == ApplyShaders::Wear {
                    card.approve_shaders();
                }
                self.apply_workspace(&card.name, shaders, window, cx);
            }
            Some(Pending::ClearEmbeddings(model)) => self.clear_embeddings(&model, cx),
            Some(Pending::ClearMeasuredBpm) => self.clear_measured_bpm(cx),
            Some(Pending::RemoveSubsonic(id)) => self.remove_subsonic(id, cx),
            Some(Pending::ClearListens) => {
                // The first yes means imported only where the dialog offered
                // both.
                let split = self.listens().imported > 0;
                self.clear_listens(
                    match yes {
                        Yes::First if split => rox_library::listens::Clear::Imported,
                        _ => rox_library::listens::Clear::Everything,
                    },
                    cx,
                );
            }
            None => {}
        }
    }

    /// Walks the live stack and tab entities, not the dock's `DockItem` tree,
    /// which goes stale once tabs move.
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
                    ix,
                    len,
                };
                self.tree_rows(child, depth + 1, child_slot, rows, cx);
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
        rows.push(self.panel_row(rows.len(), panel, depth, slot, cx));
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

    fn export_preset(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(preset) = rox_core::settings::layouts::resolve(&Settings::load(), name) else {
            return;
        };
        // Denoise on export too: presets saved before the store-time pass still
        // carry widened f64 tails.
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

    /// Panel config like the library's columns only reaches disk on the next
    /// dump, so flush before a workspace save.
    fn flush_workspace_layout(&self, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let _ = self.workspace_window.update(cx, |_, window, cx| {
            if let Some(ws) = ws.upgrade() {
                ws.update(cx, |this, cx| this.persist(window, cx));
            }
        });
    }

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

    fn overwrite_workspace(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_workspace_layout(cx);
        // Written back to the same file, keeping the card it already had.
        crate::workspaces::store(&crate::workspaces::snapshot(&name, &Settings::load()));
        self.workspace_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.workspace_authors = crate::workspaces::saved_authors();
        // Re-read a card open on the replaced workspace.
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

    /// Shader assets ride inside the file as encoded bytes. Past
    /// [`EXPORT_SIZE_WARN`] that goes to the log, never a cap (ADR 23).
    fn export_workspace(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mut bundle) = crate::workspaces::resolve(name) else {
            return;
        };
        // In place, so the bundle keeps its field order; a trip through
        // serde_json::Value would sort the keys.
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

    /// Deduped so an import never shadows an existing workspace. A bundle with
    /// unapproved shaders opens the apply confirm on the way in, so its code
    /// gets read out at import; backing out leaves it saved and unapproved.
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
        // The workspace's own apply strips its copy too; this one feeds the
        // no-dock fallback and the editor copy.
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
        // The workspace window can be gone; the look still applies, with no
        // dock to swap.
        if !applied {
            crate::workspaces::apply_look(&bundle, cx);
        }
        // Mirror the applied look into the editor. The apply may have flipped
        // the theme side.
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
        self.primary_layout = bundle.primary_layout.clone();
        self.mini_layout = bundle.mini_layout.clone();
        cx.notify();
    }
}

/// The value wraps in its own column: a description runs as long as its author
/// wrote it.
fn card_readout_line(label: impl Into<SharedString>, value: String) -> Div {
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
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(SharedString::from(value)))
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
    label: &'static str,
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
        .child(label)
        .when_some(controls, |d, controls| d.child(controls))
        .into_any_element()
}

fn shipped_tag() -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .text_color(palette::text_muted())
        .child(rox_i18n::t!("settings-common-built-in"))
}

fn role_chip(
    label: &'static str,
    active: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if active {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
            } else {
                d.bg(palette::bg_control())
                    .text_color(palette::text_muted())
                    .hover(|d| d.bg(palette::bg_control_hover()))
            }
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}
