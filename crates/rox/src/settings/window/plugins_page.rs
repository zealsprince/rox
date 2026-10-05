//! The Plugins settings page (ADR 30): every folder in the plugins folder and
//! every plugin record, each with its switch, and Developer mode beside a
//! switched-on one. A switched-on plugin's row unfolds to its config, how
//! many of its tracks are in the library, its synced collections and, when
//! it asks for it, scrobbling. The Plugins switch at the head of the page lets any of
//! them run, and the list shows only while it's on.
//!
//! The switch is the approving act. Turning on a folder this machine hasn't
//! approved opens the enable card first, and confirming it is the only way
//! an approval gets written.

use std::collections::{HashMap, HashSet};

use gpui_component::tooltip::Tooltip;
use rox_library::members::InLibrary;
use rox_library::projection::{FilterField, FilterSet};
use rox_plugins::host::STOPPED_AFTER_CRASHES;
use rox_plugins::{Loaded, Status};
use rox_services::plugin_library;
use rox_services::plugins::{self as host, Change};
use serde_json::Value;

use super::*;

/// The action cell here holds Developer mode and Remove beside the switch.
const PLUGIN_ACTION_W: Pixels = px(100.);

/// The guide as of this build's release, so it describes the host that's
/// running rather than whatever main has moved on to.
const GUIDE_URL: &str = concat!(
    "https://github.com/zealsprince/rox/blob/v",
    env!("CARGO_PKG_VERSION"),
    "/README_PLUGINS.md"
);

/// The page's own state, re-read when the host's generation moves.
#[derive(Default)]
pub(super) struct PluginsPage {
    generation: Option<u64>,
    folders: Vec<Loaded>,
    /// Member counts per synced collection, by plugin id.
    collections: HashMap<String, Vec<(String, usize)>>,
    in_library: HashMap<String, InLibrary>,
    /// The library projection the counts were read against.
    library_gen: Option<u64>,
    /// Text and number fields of switched-on plugins, by plugin id and key.
    inputs: HashMap<(String, String), ConfigInput>,
    syncing: HashSet<String>,
    /// A failed Sync Now, shown under its plugin until the next one.
    errors: HashMap<String, String>,
    /// A field changed since the hosts last picked up config.
    config_dirty: bool,
    /// Plugins whose details are unfolded, by id.
    open: HashSet<String>,
}

/// Search terms for the Program Folders row on both pages it sits on.
pub(super) const PROGRAM_FOLDERS_KEYWORDS: &[&str] = &[
    "program", "folder", "path", "homebrew", "bin", "ffmpeg", "python", "plugin",
];

/// The Program Folders field, on this page and beside Convert's ffmpeg row.
/// One input per page, since search shows both rows at once and an input
/// drawn twice keeps only the last one's layout for clicks.
pub(super) struct ProgramFolders {
    pub(super) plugins: Entity<InputState>,
    pub(super) convert: Entity<InputState>,
    /// Edited since the plugins folder was last rescanned for it.
    dirty: bool,
    _changes: [Subscription; 2],
}

impl ProgramFolders {
    pub(super) fn new(
        value: &str,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> ProgramFolders {
        let plugins = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));
        let convert = cx.new(|cx| InputState::new(window, cx).default_value(value.to_string()));

        let _changes = [
            Self::follow(&plugins, convert.clone(), window, cx),
            Self::follow(&convert, plugins.clone(), window, cx),
        ];

        ProgramFolders {
            plugins,
            convert,
            dirty: false,
            _changes,
        }
    }

    /// Every keystroke saves and reaches the program lookup, which is cheap.
    /// The rescan hashes every plugin folder, so it waits for the edit to end.
    fn follow(
        input: &Entity<InputState>,
        twin: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Subscription {
        cx.subscribe_in(
            input,
            window,
            move |this: &mut SettingsWindow, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();

                    // `set_value` is silent, so the twin doesn't echo back.
                    if twin.read(cx).value().as_ref() != text.as_str() {
                        twin.update(cx, |twin, cx| twin.set_value(text.clone(), window, cx));
                    }

                    rox_plugins::search::set_extra_dirs(settings::split_folders(&text));
                    Settings::update(move |s| s.program_folders = text);
                    this.program_folders.dirty = true;
                    this.ffmpeg_test = None;
                    cx.notify();
                }

                InputEvent::Blur | InputEvent::PressEnter { .. } => {
                    this.commit_program_folders(cx);
                }

                _ => {}
            },
        )
    }
}

pub(super) struct ConfigInput {
    input: Entity<InputState>,
    _changes: Subscription,
}

/// How a config property draws: the contract's subset of JSON Schema, and
/// raw JSON for anything outside it.
#[derive(Debug, PartialEq)]
enum Field {
    Text { secret: bool },
    Number { integer: bool },
    Toggle,
    Choice(Vec<Value>),
    Raw,
}

fn field(schema: &Value) -> Field {
    if let Some(options) = schema["enum"].as_array() {
        return Field::Choice(options.clone());
    }

    match schema["type"].as_str() {
        Some("string") => Field::Text {
            secret: schema["format"] == "password",
        },
        Some("number") => Field::Number { integer: false },
        Some("integer") => Field::Number { integer: true },
        Some("boolean") => Field::Toggle,
        _ => Field::Raw,
    }
}

/// `(key, schema)` for each config property, in the manifest's key order as
/// serde_json keeps it.
fn properties(folder: &Loaded) -> Vec<(String, Value)> {
    folder
        .manifest
        .as_ref()
        .and_then(|m| m.config_schema["properties"].as_object())
        .map(|props| props.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// What a value reads as in a field or a menu.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The one line a row shows about itself, most urgent first.
#[derive(Debug, PartialEq)]
enum Standing {
    Missing,
    Failed(String),
    Changed,
    Stopped(String),
    Needs(Vec<String>),
    On,
    Off,
}

fn standing(
    folder: Option<&Loaded>,
    record: Option<&PluginRecord>,
    approved: bool,
    status: Option<Status>,
) -> Standing {
    let Some(folder) = folder else {
        return Standing::Missing;
    };

    if let Some(error) = &folder.error {
        return Standing::Failed(error.clone());
    }
    if folder.manifest.is_none() {
        return Standing::Failed(String::new());
    }

    let on = record.is_some_and(|record| record.enabled);
    let was_approved = record.is_some_and(|record| !record.hash.is_empty());
    if !approved && (on || was_approved) {
        return Standing::Changed;
    }

    if on && let Some(Status::Stopped(reason)) = status {
        return Standing::Stopped(reason);
    }

    let missing: Vec<String> = folder
        .programs
        .iter()
        .filter(|(_, found)| !found)
        .map(|(program, _)| program.clone())
        .collect();
    if !missing.is_empty() {
        return Standing::Needs(missing);
    }

    match on {
        true => Standing::On,
        false => Standing::Off,
    }
}

impl Standing {
    fn text(&self) -> SharedString {
        match self {
            Standing::Missing => rox_i18n::t!("settings-plugins-standing-missing"),
            Standing::Failed(_) => rox_i18n::t!("settings-plugins-standing-failed"),
            Standing::Changed => rox_i18n::t!("settings-plugins-standing-changed"),
            Standing::Stopped(reason) if reason == STOPPED_AFTER_CRASHES => {
                rox_i18n::t!("settings-plugins-standing-stopped")
            }
            Standing::Stopped(reason) => reason.clone().into(),
            Standing::Needs(programs) => rox_i18n::t!(
                "settings-plugins-standing-needs",
                programs = programs.join(", ")
            ),
            Standing::On => rox_i18n::t!("settings-library-source-on"),
            Standing::Off => rox_i18n::t!("settings-library-source-off"),
        }
    }
}

/// What the confirm reads out before a plugin runs for the first time, or
/// again after its folder changed. Built once, when the switch is pressed.
pub(crate) struct EnableCard {
    pub(super) name: String,
    /// The folder as scanned when the card opened: its hash is what gets
    /// approved, not whatever is on disk by the time Confirm is pressed.
    folder: Loaded,
    /// Who made it and what it says it is, above the dialog's body.
    pub(super) lead: Vec<SharedString>,
    /// What it declares, the programs it uses, and what changed, below it.
    pub(super) lines: Vec<SharedString>,
}

impl EnableCard {
    fn of(folder: Loaded, record: Option<&PluginRecord>) -> Option<EnableCard> {
        let manifest = folder.manifest.clone()?;
        let mut lead: Vec<SharedString> = Vec::new();
        let mut lines: Vec<SharedString> = Vec::new();

        let mut byline = Vec::new();
        if !manifest.meta.author.trim().is_empty() {
            byline.push(
                rox_i18n::t!(
                    "workspace-byline-author",
                    author = manifest.meta.author.trim()
                )
                .to_string(),
            );
        }
        byline.push(
            rox_i18n::t!(
                "workspace-byline-version",
                version = manifest.version.as_str()
            )
            .to_string(),
        );
        lead.push(byline.join(", ").into());

        if !manifest.meta.description.trim().is_empty() {
            lead.push(manifest.meta.description.trim().to_string().into());
        }

        if let Some(source) = &manifest.capabilities.source {
            lines.push(rox_i18n::t!(
                "settings-plugins-card-source",
                label = source.label.as_str()
            ));
            if source.scrobble {
                lines.push(rox_i18n::t!("settings-plugins-card-scrobbles"));
            }
            if source.lyrics {
                lines.push(rox_i18n::t!("settings-plugins-card-lyrics"));
            }
            if source.favourites.is_some() {
                lines.push(rox_i18n::t!("settings-plugins-card-favourites"));
            }

            if !source.actions.is_empty() {
                let labels: Vec<&str> = source.actions.iter().map(|a| a.label.as_str()).collect();
                lines.push(rox_i18n::t!(
                    "settings-plugins-card-actions",
                    actions = labels.join(", ")
                ));
            }
        }

        for (program, found) in &folder.programs {
            lines.push(match found {
                true => rox_i18n::t!(
                    "settings-plugins-card-program-found",
                    program = program.as_str()
                ),
                false => rox_i18n::t!(
                    "settings-plugins-card-program-missing",
                    program = program.as_str()
                ),
            });
        }

        // Only a re-approval has something to diff against.
        let approved = record
            .map(|record| &record.approved_manifest)
            .filter(|approved| !approved.is_null());
        if let Some(approved) = approved {
            let changes = host::changes(approved, &folder.document);
            match changes.is_empty() {
                true => lines.push(rox_i18n::t!("settings-plugins-card-unchanged")),
                false => {
                    lines.push(rox_i18n::t!("settings-plugins-card-changed"));
                    lines.extend(changes.iter().map(change_line));
                }
            }
        }

        Some(EnableCard {
            name: manifest.name.clone(),
            folder,
            lead,
            lines,
        })
    }
}

fn change_line(change: &Change) -> SharedString {
    match change {
        Change::CapabilityAdded(name) => rox_i18n::t!(
            "settings-plugins-change-capability-added",
            name = name.as_str()
        ),
        Change::CapabilityRemoved(name) => rox_i18n::t!(
            "settings-plugins-change-capability-removed",
            name = name.as_str()
        ),
        Change::ProgramAdded(program) => rox_i18n::t!(
            "settings-plugins-change-program",
            program = program.as_str()
        ),
        Change::Scrobble(true) => rox_i18n::t!("settings-plugins-change-scrobble-on"),
        Change::Scrobble(false) => rox_i18n::t!("settings-plugins-change-scrobble-off"),
        Change::Entry => rox_i18n::t!("settings-plugins-change-entry"),
        Change::ActionAdded(label) => rox_i18n::t!(
            "settings-plugins-change-action-added",
            label = label.as_str()
        ),
        Change::ActionChanged(label) => rox_i18n::t!(
            "settings-plugins-change-action-changed",
            label = label.as_str()
        ),
    }
}

/// Member counts per collection, on its own connection like the Sources
/// table's stats.
fn read_collections(library: &Entity<Library>, id: &str, cx: &App) -> Vec<(String, usize)> {
    let db = library.read(cx).db_path();
    if !db.exists() {
        return Vec::new();
    }

    rox_library::store::open(&db)
        .ok()
        .and_then(|conn| rox_library::members::collections(&conn, &format!("plugin:{id}")).ok())
        .unwrap_or_default()
}

impl SettingsWindow {
    /// Off stops every plugin; each keeps its own switch for when this comes
    /// back on.
    fn set_plugins_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.plugins_enabled = on;
        Settings::update(move |s| s.plugins_enabled = on);
        settings::set_plugins_enabled(on, cx);
        host::apply(cx);
        cx.notify();
    }

    fn set_plugin_favourites(&mut self, on: bool, cx: &mut Context<Self>) {
        self.plugin_favourites = on;
        Settings::update(move |s| s.plugin_favourites = on);
        rox_services::plugin_favourites::set_enabled(&self.library, on, cx);
        cx.notify();
    }

    /// From render: re-read when the host moved, and give every text field a
    /// switched-on plugin's config shows an input to hold it.
    pub(super) fn sync_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.plugins_enabled {
            return;
        }

        if self.plugin_page.generation != Some(host::generation()) {
            self.refresh_plugins(cx);
        }

        // A Remove from Library elsewhere moves the counts without moving the host.
        if self.plugin_page.library_gen != Some(self.library.read(cx).projection_gen()) {
            self.read_plugin_counts(cx);
        }

        self.ensure_config_inputs(window, cx);
    }

    fn refresh_plugins(&mut self, cx: &mut Context<Self>) {
        // Read first, so a change landing mid-refresh moves it again.
        self.plugin_page.generation = Some(host::generation());

        let records = Settings::load().accounts.plugins;
        self.plugins = subsonic::read_plugins(&records, &self.library, cx);
        self.plugin_page.folders = host::loaded();
        self.read_plugin_counts(cx);
    }

    fn read_plugin_counts(&mut self, cx: &mut Context<Self>) {
        let library = self.library.read(cx);
        self.plugin_page.library_gen = Some(library.projection_gen());

        let ids: Vec<String> = self.plugins.iter().map(|(r, _)| r.id.clone()).collect();
        self.plugin_page.collections = ids
            .iter()
            .map(|id| (id.clone(), read_collections(&self.library, id, cx)))
            .collect();
        self.plugin_page.in_library = ids
            .into_iter()
            .map(|id| {
                let counts = plugin_library::in_library(library, &format!("plugin:{id}"));
                (id, counts)
            })
            .collect();
    }

    fn record(&self, id: &str) -> Option<&PluginRecord> {
        self.plugins
            .iter()
            .map(|(record, _)| record)
            .find(|record| record.id == id)
    }

    fn ensure_config_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut wanted = Vec::new();
        for folder in &self.plugin_page.folders {
            let Some(record) = self.record(&folder.id).filter(|r| r.enabled) else {
                continue;
            };

            for (key, schema) in properties(folder) {
                let id = (folder.id.clone(), key.clone());
                if self.plugin_page.inputs.contains_key(&id) {
                    continue;
                }

                let kind = field(&schema);
                if matches!(kind, Field::Text { .. } | Field::Number { .. }) {
                    wanted.push((id, kind, plain(&record.config[key.as_str()])));
                }
            }
        }

        for ((id, key), kind, value) in wanted {
            let secret = matches!(kind, Field::Text { secret: true });
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(secret)
                    .default_value(value)
            });

            let (plugin, name) = (id.clone(), key.clone());
            let _changes = cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        let Some(value) = parse(&kind, &text) else {
                            return;
                        };

                        host::set_config(&plugin, &name, value);
                        this.plugin_page.config_dirty = true;
                    }

                    // The host restarts with its new config once the edit ends.
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        this.commit_plugin_config(cx);
                    }

                    _ => {}
                }
            });

            self.plugin_page
                .inputs
                .insert((id, key), ConfigInput { input, _changes });
        }
    }

    /// Rescans so each plugin's Needs line follows the folders. Running
    /// plugins keep the PATH they started with. Also run when the page is
    /// left or the window closes.
    pub(super) fn commit_program_folders(&mut self, cx: &mut App) {
        if std::mem::take(&mut self.program_folders.dirty) {
            host::rescan(cx);
        }
    }

    /// Restarts any host whose config moved. Also run when the page is left
    /// or the window closes, since those end an edit without a blur.
    pub(super) fn commit_plugin_config(&mut self, cx: &mut App) {
        if std::mem::take(&mut self.plugin_page.config_dirty) {
            host::apply(cx);
        }
    }

    fn switch_plugin(&mut self, id: &str, on: bool, cx: &mut Context<Self>) {
        if !on {
            host::set_enabled(id, false, cx);
            self.refresh_plugins(cx);
            cx.notify();
            return;
        }

        let Some(folder) = self
            .plugin_page
            .folders
            .iter()
            .find(|folder| folder.id == id && folder.runs())
            .cloned()
        else {
            return;
        };

        // This exact folder was approved before: switching on asks nothing.
        if settings::plugin_approved(id, &folder.hash) {
            host::approve(&folder, cx);
            self.plugin_page.open.insert(id.to_string());
            self.refresh_plugins(cx);
        } else if let Some(card) = EnableCard::of(folder, self.record(id)) {
            self.pending = Some(Pending::EnablePlugin(Box::new(card)));
        }

        cx.notify();
    }

    /// Only ever called from the enable card's Switch On.
    pub(super) fn confirm_enable_plugin(&mut self, card: Box<EnableCard>, cx: &mut Context<Self>) {
        host::approve(&card.folder, cx);
        self.plugin_page.open.insert(card.folder.id.clone());
        self.refresh_plugins(cx);
        cx.notify();
    }

    pub(super) fn remove_plugin(&mut self, id: String, cx: &mut Context<Self>) {
        let removing = host::remove(&id, cx);
        self.plugin_page
            .inputs
            .retain(|(plugin, _), _| *plugin != id);
        self.plugin_page.open.remove(&id);
        self.refresh_plugins(cx);

        cx.spawn(async move |this, cx| {
            if let Err(e) = removing.await {
                log::warn!("plugin:{id}: removing its rows failed: {e}");
            }

            this.update(cx, |this, cx| {
                this.refresh_plugins(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    /// Whether the plugin's folder is still in the plugins folder, loadable
    /// or not.
    pub(super) fn plugin_folder_present(&self, id: &str) -> bool {
        self.plugin_page
            .folders
            .iter()
            .any(|folder| folder.id == id)
    }

    pub(super) fn plugin_name(&self, id: &str) -> String {
        let manifest_name = self
            .plugin_page
            .folders
            .iter()
            .find(|folder| folder.id == id)
            .and_then(|folder| folder.manifest.as_ref())
            .map(|manifest| manifest.name.clone());

        manifest_name
            .or_else(|| {
                self.record(id)
                    .map(|record| record.label.clone())
                    .filter(|label| !label.is_empty())
            })
            .unwrap_or_else(|| id.to_string())
    }

    fn toggle_plugin_open(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.plugin_page.open.remove(id) {
            self.plugin_page.open.insert(id.to_string());
        }
        cx.notify();
    }

    fn sync_plugin(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.plugin_page.syncing.insert(id.to_string()) {
            return;
        }
        self.plugin_page.errors.remove(id);

        let sync = host::sync_now(self.library.clone(), &format!("plugin:{id}"), cx);
        let id = id.to_string();

        cx.spawn(async move |this, cx| {
            let outcome = sync.await;

            this.update(cx, |this, cx| {
                this.plugin_page.syncing.remove(&id);
                if let Err(e) = outcome {
                    this.plugin_page.errors.insert(id.clone(), e);
                }

                let counts = read_collections(&this.library, &id, cx);
                this.plugin_page.collections.insert(id, counts);
                cx.notify();
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    pub(super) fn plugins_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let dir = settings::plugins_dir();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("settings-plugins-reveal"),
                icons::FOLDER,
                false,
                move |_, _, cx| {
                    // rox never makes the folder on its own; this is where
                    // the user asks for it. The watch picks it up.
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        log::warn!("plugins: creating {}: {e}", dir.display());
                    }
                    cx.reveal_path(&dir);
                },
            ))
            .child(small_button(
                rox_i18n::t!("settings-common-rescan"),
                icons::REFRESH_CW,
                false,
                |_, _, cx| host::rescan(cx),
            ))
            .child(small_button(
                rox_i18n::t!("settings-plugins-guide"),
                icons::EXTERNAL_LINK,
                false,
                |_, _, cx| cx.open_url(GUIDE_URL),
            ));

        // One row per folder, then one per record whose folder is gone.
        let mut ids: Vec<&str> = self
            .plugin_page
            .folders
            .iter()
            .map(|folder| folder.id.as_str())
            .collect();
        ids.extend(
            self.plugins
                .iter()
                .map(|(record, _)| record.id.as_str())
                .filter(|id| !self.plugin_page.folders.iter().any(|f| f.id == *id)),
        );

        let mut keywords = vec![
            "plugin",
            "extension",
            "source",
            "folder",
            "approve",
            "developer",
        ];
        keywords.extend(ids.iter().copied());

        let intro = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-plugins-intro"));

        let rows: Vec<AnyElement> = ids
            .iter()
            .map(|id| self.plugin_row(id, cx).into_any_element())
            .collect();
        let empty = rows.is_empty();
        let table = div().flex().flex_col().children(rows).when(empty, |d| {
            d.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-plugins-empty")),
            )
        });

        PageBody::new().section(Section::new(
            q,
            icons::PLUG,
            rox_i18n::t!("settings-page-plugins"),
            Some(controls.into_any_element()),
            |rows| {
                rows.keyed(
                    "settings-plugins-enable",
                    &["plugin", "extension", "source", "addon"],
                    panel::toggle(self.plugins_enabled, Self::set_plugins_enabled, cx),
                )
                .when(self.plugins_enabled, |rows| {
                    rows.keyed(
                        "settings-common-program-folders",
                        PROGRAM_FOLDERS_KEYWORDS,
                        Input::new(&self.program_folders.plugins).w(px(240.)),
                    )
                    .keyed(
                        "settings-plugins-favourites",
                        &["favourite", "favorite", "heart", "sync"],
                        panel::toggle(self.plugin_favourites, Self::set_plugin_favourites, cx),
                    )
                    .custom(&keywords, || intro.into_any_element())
                    .custom(&keywords, || table.into_any_element())
                })
            },
        ))
    }

    fn plugin_row(&self, id: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let folder = self.plugin_page.folders.iter().find(|f| f.id == id);
        let record = self.record(id);
        let approved = folder.is_some_and(|f| settings::plugin_approved(id, &f.hash));
        let standing = standing(folder, record, approved, host::status(id));
        let on = record.is_some_and(|r| r.enabled) && folder.is_some_and(Loaded::runs);
        let open = on && self.plugin_page.open.contains(id);

        let version = folder
            .and_then(|f| f.manifest.as_ref())
            .map(|m| m.version.clone())
            .unwrap_or_default();

        // Only a switched-on plugin has details to fold. The rest keep the
        // chevron's width so every name lines up.
        let fold = match on {
            true => {
                let plugin = id.to_string();
                icon_button(
                    match open {
                        true => icons::CHEVRON_DOWN,
                        false => icons::CHEVRON_RIGHT,
                    },
                    false,
                    cx.listener(move |this, _, _, cx| this.toggle_plugin_open(&plugin, cx)),
                )
                .keyed(SharedString::from(format!("plugin-fold-{id}")))
                .into_any_element()
            }
            false => div()
                .flex_none()
                .w(px(14.) + tokens::SPACE_XS * 2.)
                .into_any_element(),
        };

        let name = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(fold)
            .child(
                svg()
                    .path(icons::PLUG)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            .child(div().min_w_0().truncate().child(self.plugin_name(id)))
            .when(!version.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(version),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(standing.text()),
            );

        let remove = record.map(|_| {
            let key = SharedString::from(format!("plugin-remove-{id}"));
            let id = id.to_string();
            icon_button(
                icons::CLOSE,
                false,
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::RemovePlugin(id.clone()));
                    cx.notify();
                }),
            )
            .keyed(key)
        });

        let developer = on.then(|| {
            let developing = host::developing(id);
            let plugin = id.to_string();
            div()
                .id(SharedString::from(format!("plugin-developer-{id}")))
                .child(
                    icon_button(
                        icons::SQUARE_TERMINAL,
                        false,
                        cx.listener(move |this, _, _, cx| {
                            host::set_developing(&plugin, !developing);
                            this.refresh_plugins(cx);
                            cx.notify();
                        }),
                    )
                    .keyed(SharedString::from(format!("plugin-developer-button-{id}")))
                    .when(developing, |d| d.bg(palette::bg_control_active())),
                )
                .tooltip(|window, cx| {
                    Tooltip::new(rox_i18n::t!("settings-plugins-developer")).build(window, cx)
                })
        });

        let switch: Option<AnyElement> = match folder {
            Some(folder) if folder.runs() => {
                let id = id.to_string();
                Some(
                    panel::toggle(
                        on,
                        move |this: &mut Self, on, cx| this.switch_plugin(&id, on, cx),
                        cx,
                    )
                    .into_any_element(),
                )
            }
            Some(_) => Some(panel::toggle_locked(false).into_any_element()),
            None => None,
        };

        let line = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .when(!on, |row| row.text_color(palette::text_muted()))
            .child(name)
            .child(
                div()
                    .w(PLUGIN_ACTION_W)
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(tokens::SPACE_XS)
                    .children(developer)
                    .children(remove)
                    .children(switch),
            );

        let failure = match &standing {
            Standing::Failed(reason) if !reason.is_empty() => Some(reason.clone()),
            _ => None,
        };
        let sync_error = self.plugin_page.errors.get(id).cloned();
        let below = open || failure.is_some() || sync_error.is_some();

        div()
            .id(SharedString::from(format!("plugin-{id}")))
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            // Whatever hangs under the line keeps clear of the next row's.
            .when(below, |d| d.pb(tokens::SPACE_MD))
            .child(line)
            .when_some(failure, |d, reason| {
                d.child(panel::banner(
                    panel::Tone::Bad,
                    rox_i18n::t!("settings-plugins-failed-title"),
                    vec![reason.into()],
                ))
            })
            .when_some(sync_error, |d, error| {
                d.child(panel::banner(
                    panel::Tone::Bad,
                    rox_i18n::t!("settings-plugins-sync-failed"),
                    vec![error.into()],
                ))
            })
            .when(open, |d| {
                d.children(folder.map(|folder| self.plugin_details(folder, cx)))
            })
    }

    /// Under a switched-on plugin: scrobbling and lyrics if it asks, its
    /// config, what it has in the library, and its synced collections.
    fn plugin_details(&self, folder: &Loaded, cx: &mut Context<Self>) -> Div {
        let id = folder.id.clone();
        let record = self.record(&id);
        let config = record.map(|r| r.config.clone()).unwrap_or(Value::Null);

        let cap = folder
            .manifest
            .as_ref()
            .and_then(|m| m.capabilities.source.as_ref());
        let declares_scrobble = cap.is_some_and(|cap| cap.scrobble);
        let declares_lyrics = cap.is_some_and(|cap| cap.lyrics);

        let mut body = div().flex().flex_col().gap(tokens::SPACE_SM);
        // Each row under its own name, so its switch keeps its own focus.
        let row = |name: String| div().id(SharedString::from(format!("plugin-{id}-{name}")));

        if declares_scrobble {
            let scrobble = record.is_some_and(|r| r.scrobble);
            let plugin = id.clone();
            body = body.child(row("scrobble".into()).child(panel::setting_row(
                rox_i18n::t!("settings-plugins-scrobble"),
                rox_i18n::try_translate("settings-plugins-scrobble.description"),
                panel::toggle(
                    scrobble,
                    move |this: &mut Self, on, cx| {
                        host::set_scrobble(&plugin, on);
                        this.refresh_plugins(cx);
                        cx.notify();
                    },
                    cx,
                ),
            )));
        }

        if declares_lyrics {
            let lyrics = record.is_some_and(|r| r.lyrics);
            let plugin = id.clone();
            body = body.child(row("lyrics".into()).child(panel::setting_row(
                rox_i18n::t!("settings-plugins-lyrics"),
                rox_i18n::try_translate("settings-plugins-lyrics.description"),
                panel::toggle(
                    lyrics,
                    move |this: &mut Self, on, cx| {
                        host::set_lyrics(&plugin, on);
                        this.refresh_plugins(cx);
                        cx.notify();
                    },
                    cx,
                ),
            )));
        }

        for (key, schema) in properties(folder) {
            let title = schema["title"]
                .as_str()
                .filter(|title| !title.is_empty())
                .unwrap_or(&key)
                .to_string();
            let description = schema["description"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| SharedString::from(text.to_string()));
            let current = &config[key.as_str()];

            let control = self.config_control(&id, &key, &schema, current, cx);
            body = body.child(row(format!("config-{key}")).child(panel::setting_row(
                title,
                description,
                control,
            )));
        }

        body.child(self.library_block(&id))
            .child(self.synced_block(&id, record, cx))
    }

    /// How many of the plugin's tracks are in the library, with a way to see
    /// them there.
    fn library_block(&self, id: &str) -> Div {
        let counts = self
            .plugin_page
            .in_library
            .get(id)
            .copied()
            .unwrap_or_default();

        let source = format!("plugin:{id}");
        let state = self.state.clone();
        let workspace_window = self.workspace_window;
        let show = small_button(
            rox_i18n::t!("settings-plugins-show-in-library"),
            icons::SEARCH,
            counts.tracks == 0,
            move |_, _, cx| show_in_library(&state, &source, workspace_window, cx),
        )
        .keyed(SharedString::from(format!("plugin-{id}-show-in-library")));

        let line = |label: SharedString, count: usize| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!(
                            "settings-common-tracks-count",
                            count = count as u64
                        )),
                )
        };

        let header = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-plugins-library"));

        settings_ui::nested(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(settings_ui::block_header(header, show))
                .child(line(
                    rox_i18n::t!("settings-plugins-library-all"),
                    counts.tracks,
                ))
                .child(line(
                    rox_i18n::t!("settings-plugins-library-saved"),
                    counts.saved,
                )),
        )
    }

    fn config_control(
        &self,
        id: &str,
        key: &str,
        schema: &Value,
        current: &Value,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match field(schema) {
            Field::Text { .. } | Field::Number { .. } => {
                match self
                    .plugin_page
                    .inputs
                    .get(&(id.to_string(), key.to_string()))
                {
                    Some(field) => Input::new(&field.input).w(px(240.)).into_any_element(),
                    // Made on the next render.
                    None => div().into_any_element(),
                }
            }

            Field::Toggle => {
                let (plugin, name) = (id.to_string(), key.to_string());
                panel::toggle(
                    current.as_bool().unwrap_or(false),
                    move |this: &mut Self, on, cx| {
                        host::set_config(&plugin, &name, Value::Bool(on));
                        this.refresh_plugins(cx);
                        host::apply(cx);
                        cx.notify();
                    },
                    cx,
                )
                .into_any_element()
            }

            Field::Choice(options) => {
                let host_entity = cx.entity().downgrade();
                let (plugin, name) = (id.to_string(), key.to_string());
                let picked = current.clone();

                settings_ui::select_field(
                    SharedString::from(format!("plugin-{id}-{key}")),
                    plain(current),
                    current.is_null(),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for option in &options {
                        let (this, plugin, name, value) = (
                            host_entity.clone(),
                            plugin.clone(),
                            name.clone(),
                            option.clone(),
                        );
                        menu = menu.item(
                            PopupMenuItem::new(plain(option))
                                .checked(*option == picked)
                                .on_click(move |_, _, cx| {
                                    host::set_config(&plugin, &name, value.clone());
                                    host::apply(cx);
                                    if let Some(this) = this.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.refresh_plugins(cx);
                                            cx.notify();
                                        });
                                    }
                                }),
                        );
                    }

                    menu
                })
                .into_any_element()
            }

            Field::Raw => readout(match current.is_null() {
                true => schema.to_string(),
                false => current.to_string(),
            })
            .into_any_element(),
        }
    }

    fn synced_block(&self, id: &str, record: Option<&PluginRecord>, cx: &mut Context<Self>) -> Div {
        let syncing = self.plugin_page.syncing.contains(id);
        let plugin = id.to_string();
        let sync = small_button(
            rox_i18n::t!("settings-plugins-sync-now"),
            icons::REFRESH_CW,
            syncing,
            cx.listener(move |this, _, _, cx| this.sync_plugin(&plugin, cx)),
        );

        let counts = self.plugin_page.collections.get(id);
        let synced = record.map(|r| r.synced.as_slice()).unwrap_or_default();

        let lines: Vec<Div> =
            synced
                .iter()
                .map(|collection| {
                    let count = counts
                        .and_then(|counts| counts.iter().find(|(c, _)| *c == collection.id))
                        .map(|(_, count)| *count)
                        .unwrap_or(0);
                    let title = match collection.title.is_empty() {
                        true => collection.id.clone(),
                        false => collection.title.clone(),
                    };

                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_MD)
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(div().flex_none().text_color(palette::text_muted()).child(
                            rox_i18n::t!("settings-common-tracks-count", count = count as u64),
                        ))
                })
                .collect();
        let none = lines.is_empty();

        let header = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-plugins-synced"));

        settings_ui::nested(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(settings_ui::block_header(header, sync))
                .children(lines)
                .when(none, |d| {
                    d.child(coverage_note(
                        rox_i18n::t!("settings-plugins-synced-none").to_string(),
                    ))
                }),
        )
    }
}

/// Narrow the shared search to the plugin's source alone and raise the
/// workspace, where every panel following that search shows what it has.
fn show_in_library(state: &AppState, source: &str, workspace: AnyWindowHandle, cx: &mut App) {
    let mut filter = FilterSet::default();
    filter.toggle(FilterField::Source, source);

    state.query.update(cx, |query, cx| {
        query.set(String::new(), cx);
        query.set_filter(filter, cx);
    });

    workspace
        .update(cx, |_, window, _| window.activate_window())
        .ok();
}

/// None leaves the stored value alone: a number field mid-edit ("1.") that
/// doesn't parse yet.
fn parse(kind: &Field, text: &str) -> Option<Value> {
    match kind {
        Field::Number { .. } if text.trim().is_empty() => Some(Value::Null),
        Field::Number { integer: true } => text.trim().parse::<i64>().ok().map(Value::from),
        Field::Number { integer: false } => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        _ => Some(Value::String(text.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_contract_subset_draws_as_rows() {
        assert_eq!(
            field(&json!({ "type": "string" })),
            Field::Text { secret: false }
        );
        assert_eq!(
            field(&json!({ "type": "string", "format": "password" })),
            Field::Text { secret: true }
        );
        assert_eq!(
            field(&json!({ "type": "integer" })),
            Field::Number { integer: true }
        );
        assert_eq!(
            field(&json!({ "type": "number" })),
            Field::Number { integer: false }
        );
        assert_eq!(field(&json!({ "type": "boolean" })), Field::Toggle);
        assert_eq!(
            field(&json!({ "type": "string", "enum": ["a", "b"] })),
            Field::Choice(vec![json!("a"), json!("b")])
        );

        for other in [
            json!({ "type": "array" }),
            json!({ "type": "object" }),
            json!({}),
        ] {
            assert_eq!(field(&other), Field::Raw, "{other}");
        }
    }

    #[test]
    fn a_number_field_writes_only_what_parses() {
        let integer = Field::Number { integer: true };
        assert_eq!(parse(&integer, "42"), Some(json!(42)));
        assert_eq!(parse(&integer, "4.2"), None);
        assert_eq!(parse(&integer, " "), Some(Value::Null));

        let number = Field::Number { integer: false };
        assert_eq!(parse(&number, "0.5"), Some(json!(0.5)));
        assert_eq!(parse(&number, "x"), None);

        let text = Field::Text { secret: true };
        assert_eq!(parse(&text, " pw "), Some(json!(" pw ")));
    }

    fn folder(error: Option<&str>, programs: &[(&str, bool)]) -> Loaded {
        let manifest = rox_plugins::manifest::parse(
            r#"{ "id": "tones", "name": "Tones", "version": "1", "api": 1,
                 "entry": { "native": { "linux-x86_64": "a" } } }"#,
        )
        .unwrap();

        Loaded {
            id: "tones".into(),
            dir: "/plugins/tones".into(),
            manifest: Some(manifest),
            document: Value::Null,
            hash: "beef".into(),
            programs: programs
                .iter()
                .map(|(program, found)| (program.to_string(), *found))
                .collect(),
            error: error.map(str::to_string),
            icon: None,
            action_icons: Vec::new(),
        }
    }

    fn record(enabled: bool, hash: &str) -> PluginRecord {
        PluginRecord {
            id: "tones".into(),
            enabled,
            hash: hash.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_row_says_the_most_urgent_thing_first() {
        let ok = folder(None, &[]);
        let on = record(true, "beef");

        assert_eq!(standing(None, Some(&on), true, None), Standing::Missing);
        assert_eq!(
            standing(Some(&folder(Some("bad"), &[])), Some(&on), true, None),
            Standing::Failed("bad".into())
        );
        assert_eq!(
            standing(Some(&ok), Some(&record(false, "cafe")), false, None),
            Standing::Changed
        );
        assert_eq!(
            standing(
                Some(&ok),
                Some(&on),
                true,
                Some(Status::Stopped(STOPPED_AFTER_CRASHES.into()))
            ),
            Standing::Stopped(STOPPED_AFTER_CRASHES.into())
        );
        assert_eq!(
            standing(
                Some(&folder(None, &[("dl", false), ("ok", true)])),
                None,
                false,
                None
            ),
            Standing::Needs(vec!["dl".into()])
        );
        assert_eq!(standing(Some(&ok), Some(&on), true, None), Standing::On);
        assert_eq!(standing(Some(&ok), None, false, None), Standing::Off);
    }

    #[test]
    fn a_folder_never_approved_reads_off_not_changed() {
        let ok = folder(None, &[]);

        assert_eq!(
            standing(Some(&ok), Some(&record(false, "")), false, None),
            Standing::Off
        );
    }
}
