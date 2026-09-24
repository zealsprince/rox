//! The tag editor window, opened on a selection. One shared form covers the
//! selection: a field every file agrees on shows its value, a split one shows
//! empty over a "multiple values" placeholder, and only fields the user moves
//! write anything. Table mode swaps the form for a row of cells per track,
//! where per-track fields stay editable.
//!
//! Baselines come off each file through the writer's read, so every save
//! diffs per file against what that file has and commits through the atomic
//! layer. A save then applies to the catalog in one batch and re-reads the
//! written files so their rows converge with the disk. Last edit wins between
//! the form and the cells (ADR 18).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, ClickEvent, Context, Div, ElementId, Entity, FocusHandle, Focusable as _, Global,
    KeyBinding, MouseButton, MouseDownEvent, ScrollHandle, SharedString, Stateful, Subscription,
    WeakEntity, Window, WindowHandle, actions, div, prelude::*, px, size, svg,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Root, Sizable, Size};

use rox_library::cue::{TrackKey, local};
use rox_library::rating;
use rox_library::writer::{self, Change, Edit, Field, UnknownValue};

use crate::matching::{WindowRegistry, open_or_focus};
use crate::tags::guess;
use crate::tags::replace;
use rox_core::settings::{RatingStyle, Settings, rating_style};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers;
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::suggest;
use rox_panel_kit::ui::{
    self as settings_ui, SECTION_GAP, Seg, kbd_line, section, section_with_control,
};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// The bool marks per-track fields, which only edit with a single track
/// selected; a batch would stamp one title over every file. Each sort field
/// sits under the field it sorts and shares its bool.
const FIELDS: &[(Field, &str, bool)] = &[
    (Field::Title, "title", true),
    (Field::TitleSort, "title sort", true),
    (Field::Artist, "artist", false),
    (Field::ArtistSort, "artist sort", false),
    (Field::AlbumArtist, "album artist", false),
    (Field::AlbumArtistSort, "album artist sort", false),
    (Field::Album, "album", false),
    (Field::AlbumSort, "album sort", false),
    (Field::Genre, "genre", false),
    (Field::Year, "year", false),
    (Field::TrackNo, "track", true),
    (Field::DiscNo, "disc", true),
    (Field::Comment, "comment", false),
    // The writer's 0-10 number, half points included.
    (Field::Rating, "rating", false),
];

/// Commits are mostly disk, so past a handful in flight the drive is the
/// limit; the convert and analysis pools cap at the same place.
const SAVE_WORKERS: usize = 4;

/// Columns ahead of [`FIELDS`] in the full order. Width slots are positional
/// over the full order, hidden columns included, so a width survives a
/// toggle.
const LEAD: usize = 1;

/// Without a prefix, a stray tag spelled "album" would answer to the album
/// field's column and the writer would edit the album behind its row's back.
const TAG_PREFIX: &str = "tag:";

const TAG_WIDTH: f32 = 150.;

/// Most files carry none of the sort names, and all fourteen columns would
/// push the ones people came for off the right edge.
const OPT_IN_COLUMNS: &[&str] = &[
    "title sort",
    "artist sort",
    "album artist sort",
    "album sort",
];

/// Every additional tag is opt-in too, or a selection with fifteen stray
/// keys would open with fifteen surprise columns.
fn opt_in_column(key: &str) -> bool {
    key.starts_with(TAG_PREFIX) || OPT_IN_COLUMNS.contains(&key)
}

fn sort_field(label: &str) -> bool {
    OPT_IN_COLUMNS.contains(&label)
}

/// An index is the row's slot in [`FIELDS`], where its input, fill and
/// mixed flag sit too.
fn form_fields(sort_fields: bool) -> Vec<usize> {
    FIELDS
        .iter()
        .enumerate()
        .filter(|(_, (_, label, _))| sort_fields || !sort_field(label))
        .map(|(i, _)| i)
        .collect()
}

/// The labels are English literals, so a character count is the whole
/// measurement; the real advance only exists inside a paint.
const LABEL_CHAR_W: f32 = 7.5;

const LABEL_MIN_W: f32 = 84.;

/// Sized so "Album Artist Sort" holds one line.
fn label_column_w(rows: &[usize]) -> f32 {
    rows.iter()
        .map(|i| title_case(FIELDS[*i].1).chars().count() as f32 * LABEL_CHAR_W)
        .fold(LABEL_MIN_W, f32::max)
        .ceil()
}

/// A fill lands whether the sort rows are folded or not, so it opens them.
fn fills_sort_field(values: &[(Field, String)]) -> bool {
    values.iter().any(|(field, value)| {
        !value.trim().is_empty()
            && FIELDS
                .iter()
                .any(|(f, label, _)| f == field && sort_field(label))
    })
}

/// The table's half of the same rule: turn on the sort columns a fill wrote
/// to.
fn sort_columns_to_show(values: &[(Field, String)], shown: &HashSet<String>) -> Vec<&'static str> {
    FIELDS
        .iter()
        .filter(|(field, label, _)| {
            sort_field(label)
                && !shown.contains(*label)
                && values
                    .iter()
                    .any(|(f, value)| f == field && !value.trim().is_empty())
        })
        .map(|(_, label, _)| *label)
        .collect()
}

/// An ordinary field shows unless hidden; an opt-in one only while shown.
fn column_shown(key: &str, hidden: &HashSet<String>, shown: &HashSet<String>) -> bool {
    if opt_in_column(key) {
        shown.contains(key)
    } else {
        !hidden.contains(key)
    }
}

fn title_case(label: &str) -> String {
    label
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn column_keys() -> impl Iterator<Item = &'static str> {
    std::iter::once("file").chain(FIELDS.iter().map(|(_, label, _)| *label))
}

/// Where its width is stored, whether the column shows or not.
fn canonical_ix(key: &str) -> Option<usize> {
    column_keys().position(|k| k == key)
}

fn default_widths() -> Vec<f32> {
    std::iter::once(220.)
        .chain(FIELDS.iter().map(|(field, _, _)| match field {
            Field::Year | Field::TrackNo | Field::DiscNo => 64.,
            Field::Rating => 96.,
            _ => 150.,
        }))
        .collect()
}

/// Widths are positional, so a set from another column order can't be read
/// straight. The layout from before the four sort columns is translated:
/// skip the sort columns and walk both. Any other length falls back to the
/// defaults whole.
fn placed_widths(saved: &[f32]) -> Option<Vec<f32>> {
    let defaults = default_widths();
    if saved.len() == defaults.len() {
        return Some(saved.to_vec());
    }
    if saved.len() + OPT_IN_COLUMNS.len() != defaults.len() {
        return None;
    }
    let mut old = saved.iter().copied();
    Some(
        column_keys()
            .zip(defaults)
            .map(|(key, default)| {
                if sort_field(key) {
                    default
                } else {
                    old.next().unwrap_or(default)
                }
            })
            .collect(),
    )
}

/// A binary payload's column is read-only, like its row.
#[derive(Clone)]
struct TagColumn {
    key: String,
    name: SharedString,
    text: bool,
}

/// Resolved once per column, so a tag column can't fall through to the file
/// column's branch and sort the grid by file name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColumnKind {
    File,
    Field(usize),
    Tag(usize),
}

/// None only for a hand-edited settings key; its cells draw empty.
fn column_kind(key: &str, tags: &[TagColumn]) -> Option<ColumnKind> {
    if let Some(tag) = key.strip_prefix(TAG_PREFIX) {
        return tags
            .iter()
            .position(|column| column.key == tag)
            .map(ColumnKind::Tag);
    }
    if key == "file" {
        return Some(ColumnKind::File);
    }
    FIELDS
        .iter()
        .position(|(_, label, _)| *label == key)
        .map(ColumnKind::Field)
}

/// Tags after the fixed columns, so a column toggled back lands where it
/// left.
fn column_rank(key: &str, tags: &[TagColumn]) -> Option<usize> {
    if let Some(ix) = canonical_ix(key) {
        return Some(ix);
    }
    let tag = key.strip_prefix(TAG_PREFIX)?;
    let at = tags.iter().position(|column| column.key == tag)?;
    Some(LEAD + FIELDS.len() + at)
}

/// The field a tag key would edit behind that field's back, if any. The
/// writer maps keys like TITLE onto the field's own item, so the editor
/// refuses them rather than let two surfaces write one tag. Matched on
/// letters alone. The alias list is what people type, not lofty's full map;
/// the writer stays correct either way.
fn field_owning(key: &str) -> Option<&'static str> {
    const ALIASES: &[(&str, &str)] = &[
        ("tit2", "title"),
        ("tsot", "title sort"),
        ("titlesort", "title sort"),
        ("titlesortorder", "title sort"),
        ("tpe1", "artist"),
        ("tsop", "artist sort"),
        ("artistsort", "artist sort"),
        ("artistsortorder", "artist sort"),
        ("tpe2", "album artist"),
        ("albumartist", "album artist"),
        ("tso2", "album artist sort"),
        ("albumartistsort", "album artist sort"),
        ("albumartistsortorder", "album artist sort"),
        ("talb", "album"),
        ("tsoa", "album sort"),
        ("albumsort", "album sort"),
        ("albumsortorder", "album sort"),
        ("tcon", "genre"),
        ("tdrc", "year"),
        ("tyer", "year"),
        ("date", "year"),
        ("trck", "track"),
        ("tracknumber", "track"),
        ("tpos", "disc"),
        ("discnumber", "disc"),
        ("partofset", "disc"),
        ("comm", "comment"),
        ("popm", "rating"),
    ];
    let letters = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let key = letters(key);
    if key.is_empty() {
        return None;
    }
    FIELDS
        .iter()
        .map(|(_, label, _)| *label)
        .find(|label| letters(label) == key)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| *alias == key)
                .map(|(_, label)| *label)
        })
}

/// None for a blank key, or one a field already owns.
fn tag_key_of(raw: &str) -> Option<String> {
    let key = raw.trim();
    (!key.is_empty() && field_owning(key).is_none()).then(|| key.to_owned())
}

/// A key read off a file is kept byte for byte, with neither refusal: a
/// file can carry a TXXX called ALBUMARTISTSORT, and trimming or refusing
/// it would draw the row and silently skip it at save.
fn file_tag_key(key: &str) -> Option<String> {
    (!key.is_empty()).then(|| key.to_owned())
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum TagIntent {
    Keep,
    Drop,
    Set(String),
}

/// The later row wins, as the writer's in-order apply would anyway. A row
/// asking for nothing never erases one that asks for something.
fn fold_tag_intents(intents: Vec<(String, TagIntent)>) -> Vec<(String, TagIntent)> {
    let mut out: Vec<(String, TagIntent)> = Vec::with_capacity(intents.len());
    for (key, intent) in intents {
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(folded) => {
                if intent != TagIntent::Keep {
                    folded.1 = intent;
                }
            }
            None => out.push((key, intent)),
        }
    }
    out
}

/// None when the file already spells it that way, the row asks nothing, or
/// a removal names a key the file never had.
fn tag_change_for(
    key: &str,
    intent: &TagIntent,
    baseline: &[(String, UnknownValue)],
) -> Option<Change> {
    let value = match intent {
        TagIntent::Keep => return None,
        TagIntent::Drop => None,
        TagIntent::Set(value) => (!value.is_empty()).then(|| value.clone()),
    };
    let carried = baseline.iter().any(|(k, _)| k == key);
    let current = baseline.iter().find_map(|(k, v)| match v {
        UnknownValue::Text(text) if k == key => Some(text.as_str()),
        _ => None,
    });
    match &value {
        None if !carried => return None,
        Some(v) if current == Some(v.as_str()) => return None,
        _ => {}
    }
    Some(Change {
        field: Field::Unknown(key.to_owned()),
        value,
    })
}

/// ADR 18's last-edit-wins rule for one cell. `form` is the form's drifted
/// value, `seed` what the cell last took from a fold, `base` the file's
/// baseline. None leaves a cell the user moved alone.
fn fold_cell(current: &str, seed: &str, base: &str, form: Option<&str>) -> Option<SharedString> {
    if current != seed {
        return None;
    }
    Some(SharedString::from(form.unwrap_or(base).to_owned()))
}

/// A tag the file never carried and one the user emptied both read as "",
/// which is what makes an untouched empty row cost nothing.
fn baseline_value<'a>(baseline: &'a [(Field, String)], field: &Field) -> &'a str {
    baseline
        .iter()
        .find(|(f, _)| f == field)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// Multi-value tags count their first item, the one the writer's verify
/// reads back.
fn shared_value(field: &Field, baselines: &[Vec<(Field, String)>]) -> (SharedString, bool) {
    let mut values = baselines.iter().map(|fields| baseline_value(fields, field));
    let first = values.next().unwrap_or_default();
    let mixed = values.any(|v| v != first);
    let value = if mixed {
        SharedString::default()
    } else {
        SharedString::from(first.to_owned())
    };
    (value, mixed)
}

fn change_for(field: &Field, value: String, baseline: &[(Field, String)]) -> Option<Change> {
    if value == baseline_value(baseline, field) {
        return None;
    }
    Some(Change {
        field: field.clone(),
        value: (!value.is_empty()).then_some(value),
    })
}

/// Empty too for a binary payload or a failed read.
fn tag_baseline_value(baseline: Option<&Vec<(String, UnknownValue)>>, key: &str) -> String {
    baseline
        .into_iter()
        .flatten()
        .find_map(|(k, value)| match value {
            UnknownValue::Text(text) if k == key => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// [`fold_cell`] over one column. `reseed` is the grid's first build. Returns
/// whether the form value had drifted.
#[allow(clippy::too_many_arguments)]
fn fold_column(
    form_value: &str,
    filled: &str,
    bases: &[String],
    cells: &[Entity<InputState>],
    seeds: &mut [SharedString],
    reseed: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let drifted = form_value != filled;
    // Re-seeding a quiet column would push its cells back to the disk
    // baseline, wiping values an earlier fold brought in.
    if !reseed && !drifted {
        return false;
    }
    let form = drifted.then_some(form_value);
    for ((cell, base), seed) in cells.iter().zip(bases).zip(seeds) {
        let current = cell.read(cx).value().clone();
        let Some(target) = fold_cell(&current, seed, base, form) else {
            continue;
        };
        if current != target {
            let value = target.clone();
            cell.update(cx, |cell, cx| cell.set_value(value, window, cx));
        }
        *seed = target;
    }
    drifted
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn field_placeholder(field: &Field) -> &'static str {
    match field {
        Field::Rating => "0-10",
        _ => "",
    }
}

/// A click writes the display number into the input, so the diff and save
/// paths see it like typing.
fn rating_field(input: &Entity<InputState>, cx: &App) -> Div {
    let current = rating::parse_display(input.read(cx).value().trim()).unwrap_or(0);
    let input = input.clone();
    // Keyed on the input's entity id, unique per editor row.
    let key = input.entity_id().as_u64();
    rox_panel_api::rating_ui::control(key, current, move |value, window, cx| {
        let text = if value == 0 {
            String::new()
        } else {
            rating::display(value)
        };
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    })
}

fn arm_chip(
    id: impl Into<ElementId>,
    on: bool,
    label: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(tokens::SPACE_XS)
        .py(px(1.))
        .rounded(tokens::RADIUS)
        .text_xs()
        .cursor_pointer()
        .map(|d| {
            if on {
                d.text_color(palette::accent())
            } else {
                d.text_color(palette::text_muted())
                    .hover(|d| d.text_color(palette::text()))
            }
        })
        .child(label)
        .on_click(on_click)
}

fn switch_row(
    id: &'static str,
    on: bool,
    label: SharedString,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_XS)
        .text_xs()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, on_mouse_down)
        .child(settings_ui::checkbox(on))
        .child(div().text_color(palette::text_muted()).child(label))
}

actions!(tag_editor, [FieldTab, FieldTabPrev, Save]);

const CONTEXT: &str = "TagEditor";

/// The tab pair scopes to the field wrappers, deeper than the root's own
/// tab bindings, so in a tag field tab takes the open suggestion before it
/// moves.
///
/// Enter is bound on the root, so it saves from anywhere. An open suggestion
/// menu swallows the first press.
pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", FieldTab, Some("TagField")),
        KeyBinding::new("shift-tab", FieldTabPrev, Some("TagField")),
        KeyBinding::new("enter", Save, Some(CONTEXT)),
    ]
}

/// Route enter to the completion menu, not the input's Enter action: with no
/// menu open that emits PressEnter, which saves and closes the window.
fn take_suggestion(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    input.update(cx, |state, cx| {
        state.handle_action_for_context_menu(Box::new(Enter { secondary: false }), window, cx);
    });
}

fn accept_then_focus(
    input: &Entity<InputState>,
    target: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    take_suggestion(input, window, cx);
    window.focus(target);
    // Accepting propagates, which would let the root's tab binding move focus
    // a second time.
    cx.stop_propagation();
}

/// Keyed by the sorted ids, so an edit in progress is focused rather than
/// twinned.
#[derive(Default)]
struct OpenTagEditors(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenTagEditors {}

impl WindowRegistry for OpenTagEditors {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    open_or_focus::<OpenTagEditors>(
        key,
        move |cx| {
            let (width, height) = Settings::load()
                .windows
                .tag_editor
                .filter(|s| s.width >= 400. && s.height >= 300.)
                .map(|s| (s.width, s.height))
                .unwrap_or((1400., 680.));
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("tags-editor-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| TagEditor::new(state, ids, window, cx)),
            )
        },
        cx,
    );
}

/// `sub` says which cue track of the file this is. The title only names the
/// track in errors.
struct TrackRow {
    path: PathBuf,
    sub: u16,
    title: SharedString,
}

enum FileRead {
    Unsupported,
    Read {
        fields: Result<Vec<(Field, String)>, String>,
        unknown: Result<Vec<(String, UnknownValue)>, String>,
    },
}

/// "Additional" rather than "unknown": after the add button, the list holds
/// rows nobody failed to recognize.
struct AdditionalTags {
    rows: Vec<AdditionalRow>,
    /// Rows off the files lead and get table columns; authored rows follow and
    /// are batch-only until they're on disk.
    columns: usize,
    /// Save leaves those files' additional tags alone: nothing safe to diff.
    failed: usize,
    files: usize,
}

struct AdditionalRow {
    /// Empty on an authored row until its key input says otherwise.
    key: String,
    label: SharedString,
    /// None on a row read off a file, whose key is fixed.
    key_input: Option<Entity<InputState>>,
    /// An edit arms by drifting from this.
    initial: SharedString,
    input: Option<Entity<InputState>>,
    binary: Option<SharedString>,
    files: usize,
    removed: bool,
    /// The replace panel is only offered where there's a split to resolve.
    mixed: bool,
}

impl AdditionalRow {
    fn key(&self, cx: &App) -> Option<String> {
        match &self.key_input {
            Some(input) => tag_key_of(&input.read(cx).value()),
            None => file_tag_key(&self.key),
        }
    }
}

#[derive(Clone)]
enum TagCell {
    Edit(Entity<InputState>),
    /// Read-only: the size this file carries, blank where it has none.
    Fixed(SharedString),
}

/// Rows are only ever appended, so a place stays good while the panel is up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplaceTarget {
    Field(usize),
    Tag(usize),
}

pub struct TagEditor {
    library: Entity<Library>,
    tracks: Vec<TrackRow>,
    /// None until every read is in, and save stays inert without it.
    baselines: Option<Vec<Vec<(Field, String)>>>,
    /// A field arms by drifting from this.
    filled: Vec<SharedString>,
    mixed: Vec<bool>,
    /// A mixed field is empty over its placeholder, so an empty input can't
    /// mean "wipe this on every file". This flag does.
    cleared: Vec<bool>,
    inputs: Vec<Entity<InputState>>,
    table: bool,
    /// Built on the first switch to table mode.
    cells: Option<Vec<Vec<Entity<InputState>>>>,
    /// The delegate shares the cell entities, so save reads what the table shows.
    grid: Option<Entity<TableState<CellGrid>>>,
    /// A hidden column keeps its cells, so nothing typed is lost to a toggle.
    hidden: HashSet<String>,
    /// Tag columns are stored under their `tag:` key, so a key survives
    /// selections that don't carry it.
    shown: HashSet<String>,
    /// Per file like a field, so fixing one file's stray key doesn't stamp the
    /// batch.
    tag_cells: Option<Vec<Vec<TagCell>>>,
    /// A cell still on its seed follows re-seeds; one the user moved is theirs.
    seeds: Vec<Vec<SharedString>>,
    tag_seeds: Vec<Vec<SharedString>>,
    sort_fields: bool,
    guess: bool,
    /// Remembered across editors: a library tends to one naming scheme.
    pattern: Entity<InputState>,
    replace: Option<ReplaceTarget>,
    /// The switches are remembered across editors, the boxes aren't.
    find: Entity<InputState>,
    replacement: Entity<InputState>,
    replace_regex: bool,
    replace_ignore_case: bool,
    additional: Option<AdditionalTags>,
    /// None where the read failed; save leaves that file's tags alone.
    additional_baselines: Vec<Option<Vec<(String, UnknownValue)>>>,
    additional_open: bool,
    /// Those files say so instead of showing a parse error over a dead form.
    unsupported: usize,
    error: Option<SharedString>,
    saving: bool,
    /// One enter can reach [`Self::save`] twice (the input's binding and the
    /// root's), and an empty save closes on the first without raising `saving`.
    saved: bool,
    save_done: usize,
    save_total: usize,
    scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    /// This window pumps its own frames, so the backdrop needs its own wake.
    _backdrop_changed: Subscription,
}

impl TagEditor {
    fn new(state: AppState, ids: Vec<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A file the projection misses still edits, its name standing in.
        let projection = state.library.read(cx).projection().cloned();
        let tracks = {
            let library = state.library.read(cx);
            let row_of: HashMap<i64, u32> = projection
                .as_ref()
                .map(|projection| {
                    projection
                        .db_id
                        .iter()
                        .enumerate()
                        .filter(|(row, _)| !projection.is_dead(*row as u32))
                        .map(|(row, &id)| (id, row as u32))
                        .collect()
                })
                .unwrap_or_default();
            let mut tracks = Vec::with_capacity(ids.len());
            for &id in &ids {
                let Some(path) = library
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop())
                else {
                    continue;
                };
                let resolved = projection.as_ref().and_then(|projection| {
                    let row = *row_of.get(&id)?;
                    let v = projection.resolve(row);
                    Some((v.title.to_owned(), v.sub))
                });
                let (title, sub) = resolved.unwrap_or_else(|| {
                    let title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    (title, 0)
                });
                tracks.push(TrackRow {
                    path,
                    sub,
                    title: title.into(),
                });
            }
            tracks
        };
        let inputs: Vec<Entity<InputState>> = FIELDS
            .iter()
            .map(|(field, _, _)| {
                cx.new(|cx| {
                    let mut input =
                        InputState::new(window, cx).placeholder(field_placeholder(field));
                    if *field == Field::Rating {
                        input = input.validate(|s, _| {
                            s.trim().is_empty() || rating::parse_display(s).is_some()
                        });
                    }
                    input.lsp.completion_provider = suggest::provider(&state.library, field, cx);
                    input
                })
            })
            .collect();
        let mut _input_events: Vec<Subscription> = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.save(window, cx),
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                )
            })
            .collect();
        // Enter in the pattern applies the guesses, not a save that would close
        // the window.
        let saved_pattern = Settings::load()
            .windows
            .tag_editor
            .map(|s| s.pattern)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "%artist% - %title%".to_owned());
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(saved_pattern)
                .placeholder("%artist% - %title%")
        });
        _input_events.push(cx.subscribe_in(
            &pattern,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.apply_guesses(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        ));
        window.focus(&inputs[0].read(cx).focus_handle(cx));
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs remove_window, so persist here too.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist_frame(window, cx));
            }
            true
        });
        let table = tracks.len() > 1;
        // Prune keys that stopped being columns, and any key in the wrong set.
        let saved = Settings::load().windows.tag_editor;
        let hidden: HashSet<String> = saved
            .as_ref()
            .map(|s| s.hidden.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|key| canonical_ix(key).is_some() && !opt_in_column(key))
            .collect();
        let shown: HashSet<String> = saved
            .as_ref()
            .map(|s| s.shown.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|key| opt_in_column(key))
            .collect();
        let sort_fields = saved.as_ref().is_some_and(|s| s.sort_fields);
        let find = cx.new(|cx| InputState::new(window, cx));
        let replacement = cx.new(|cx| InputState::new(window, cx));
        for input in [&find, &replacement] {
            _input_events.push(cx.subscribe_in(
                input,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.apply_replace(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ));
        }
        let replace_regex = saved.as_ref().is_some_and(|s| s.replace_regex);
        let replace_ignore_case = saved.as_ref().is_some_and(|s| s.replace_ignore_case);
        let this = TagEditor {
            library: state.library,
            tracks,
            baselines: None,
            filled: Vec::new(),
            mixed: Vec::new(),
            cleared: vec![false; FIELDS.len()],
            inputs,
            table,
            cells: None,
            grid: None,
            hidden,
            shown,
            tag_cells: None,
            seeds: Vec::new(),
            tag_seeds: Vec::new(),
            sort_fields,
            guess: false,
            pattern,
            replace: None,
            find,
            replacement,
            replace_regex,
            replace_ignore_case,
            additional: None,
            additional_baselines: Vec::new(),
            additional_open: false,
            unsupported: 0,
            error: None,
            saving: false,
            saved: false,
            save_done: 0,
            save_total: 0,
            scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.read_baselines(window, cx);
        this
    }

    /// One unreadable file blocks the whole save: nothing safe to diff it
    /// against. The additional tags read on the same hop.
    fn read_baselines(&self, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.tracks.iter().map(|track| track.path.clone()).collect();
        cx.spawn_in(window, async move |this, cx| {
            let reads = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .map(|path| {
                            if !writer::supported(path) {
                                return FileRead::Unsupported;
                            }
                            FileRead::Read {
                                fields: writer::read(path),
                                unknown: writer::read_unknown(path),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.unsupported = reads
                    .iter()
                    .filter(|read| matches!(read, FileRead::Unsupported))
                    .count();
                if this.unsupported > 0 {
                    cx.notify();
                    return;
                }
                this.additional_baselines = reads
                    .iter()
                    .map(|read| match read {
                        FileRead::Read {
                            unknown: Ok(rows), ..
                        } => Some(rows.clone()),
                        _ => None,
                    })
                    .collect();
                this.additional = Some(build_additional(&reads, window, cx));
                let mut baselines = Vec::with_capacity(reads.len());
                for (read, track) in reads.into_iter().zip(&this.tracks) {
                    let FileRead::Read { fields, .. } = read else {
                        continue;
                    };
                    match fields {
                        Ok(fields) => baselines.push(fields),
                        Err(e) => {
                            this.error = Some(format!("{}: {e}", track.title).into());
                            cx.notify();
                            return;
                        }
                    }
                }
                this.fill(baselines, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn toggle_additional(&mut self, cx: &mut Context<Self>) {
        self.additional_open = !self.additional_open;
        cx.notify();
    }

    fn toggle_remove_additional(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(row) = self
            .additional
            .as_mut()
            .and_then(|additional| additional.rows.get_mut(i))
        {
            row.removed = !row.removed;
            cx.notify();
        }
    }

    /// An authored row is batch-only; it gets a column on the next open, once
    /// it's on disk.
    fn add_tag_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.additional.is_none() {
            return;
        }
        let key_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rox_i18n::t!("tags-editor-tag-key-placeholder"))
        });
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("tags-editor-tag-value-placeholder"))
        });
        self._input_events.push(cx.subscribe_in(
            &key_input,
            window,
            |_: &mut Self, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        let focus = key_input.read(cx).focus_handle(cx);
        if let Some(additional) = self.additional.as_mut() {
            additional.rows.push(AdditionalRow {
                key: String::new(),
                label: SharedString::default(),
                key_input: Some(key_input),
                initial: SharedString::default(),
                input: Some(input),
                binary: None,
                files: additional.files,
                removed: false,
                mixed: false,
            });
        }
        self.additional_open = true;
        window.focus(&focus);
        cx.notify();
    }

    fn fill(
        &mut self,
        baselines: Vec<Vec<(Field, String)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for ((field, _, _), input) in FIELDS.iter().zip(&self.inputs) {
            let (value, mixed) = shared_value(field, &baselines);
            input.update(cx, |input, cx| {
                if mixed {
                    input.set_placeholder(rox_i18n::t!("tags-editor-multiple-values"), window, cx);
                }
                input.set_value(value.clone(), window, cx);
            });
            self.filled.push(value);
            self.mixed.push(mixed);
        }
        self.baselines = Some(baselines);
        if self.table {
            self.seed_cells(window, cx);
            if let Some(cells) = &self.cells {
                window.focus(
                    &cells[0][self.first_visible_field()]
                        .read(cx)
                        .focus_handle(cx),
                );
            }
        }
        cx.notify();
    }

    /// With several editors open, the last writer wins.
    fn persist_frame(&self, window: &Window, cx: &App) {
        let frame = window.window_bounds().get_bounds();
        let columns: Vec<(String, f32)> = self
            .grid
            .as_ref()
            .map(|grid| {
                grid.read(cx)
                    .delegate()
                    .columns
                    .iter()
                    .map(|column| (column.key.to_string(), column.width.into()))
                    .collect()
            })
            .unwrap_or_default();
        let mut hidden: Vec<String> = self.hidden.iter().cloned().collect();
        hidden.sort();
        let mut shown: Vec<String> = self.shown.iter().cloned().collect();
        shown.sort();
        let pattern = self.pattern.read(cx).value().to_string();
        let sort_fields = self.sort_fields;
        let (replace_regex, replace_ignore_case) = (self.replace_regex, self.replace_ignore_case);
        Settings::update(move |s| {
            let state = s.windows.tag_editor.get_or_insert_with(Default::default);
            state.width = frame.size.width.into();
            state.height = frame.size.height.into();
            // A form-only session keeps the saved widths. Tag columns write by key,
            // since the next selection carries a different set of tags.
            if !columns.is_empty() {
                // Through the same migration the table opened with, so hidden columns keep
                // their widths.
                state.columns = placed_widths(&state.columns).unwrap_or_else(default_widths);
                for (key, width) in &columns {
                    match canonical_ix(key) {
                        Some(ix) => state.columns[ix] = *width,
                        None => {
                            if let Some(tag) = key.strip_prefix(TAG_PREFIX) {
                                state.tag_columns.insert(tag.to_owned(), *width);
                            }
                        }
                    }
                }
            }
            state.hidden = hidden;
            state.shown = shown;
            state.sort_fields = sort_fields;
            state.pattern = pattern;
            state.replace_regex = replace_regex;
            state.replace_ignore_case = replace_ignore_case;
        });
    }

    fn first_visible_field(&self) -> usize {
        FIELDS
            .iter()
            .position(|(_, label, _)| column_shown(label, &self.hidden, &self.shown))
            .unwrap_or(0)
    }

    /// Never hides the last column: an empty table has no header to bring one
    /// back from.
    fn toggle_column(&mut self, key: SharedString, cx: &mut Context<Self>) {
        let Some(grid) = &self.grid else { return };
        let key = key.to_string();
        let on = column_shown(&key, &self.hidden, &self.shown);
        if !on {
            let name: SharedString = match key.strip_prefix(TAG_PREFIX) {
                Some(tag) => one_line(tag).into(),
                None => title_case(&key).into(),
            };
            let width = match canonical_ix(&key) {
                Some(canon) => default_widths()[canon],
                None => TAG_WIDTH,
            };
            let shown = grid.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                let Some(rank) = column_rank(&key, &delegate.tags) else {
                    return false;
                };
                let at = delegate
                    .columns
                    .iter()
                    .take_while(|c| {
                        column_rank(c.key.as_ref(), &delegate.tags).unwrap_or(usize::MAX) < rank
                    })
                    .count();
                let column = Column::new(key.clone(), name).width(px(width)).sortable();
                delegate.columns.insert(at, column);
                table.refresh(cx);
                true
            });
            if shown {
                if opt_in_column(&key) {
                    self.shown.insert(key);
                } else {
                    self.hidden.remove(&key);
                }
            }
        } else {
            let mut removed = false;
            grid.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                if delegate.columns.len() <= 1 {
                    return;
                }
                let Some(ix) = delegate.columns.iter().position(|c| c.key.as_ref() == key) else {
                    return;
                };
                // A hidden sort column leaves no header to clear the sort, so reset it.
                let sorted = matches!(
                    delegate.columns[ix].sort,
                    Some(ColumnSort::Ascending | ColumnSort::Descending)
                );
                delegate.columns.remove(ix);
                if sorted {
                    delegate.order = (0..delegate.cells.len()).collect();
                }
                removed = true;
                table.refresh(cx);
            });
            if removed {
                if opt_in_column(&key) {
                    self.shown.remove(&key);
                } else {
                    self.hidden.insert(key);
                }
            }
        }
        cx.notify();
    }

    fn toggle_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table {
            self.table = false;
            self.refill_form(window, cx);
            window.focus(&self.inputs[0].read(cx).focus_handle(cx));
        } else {
            if self.baselines.is_none() {
                return;
            }
            self.table = true;
            self.seed_cells(window, cx);
            if let Some(cells) = &self.cells {
                window.focus(
                    &cells[0][self.first_visible_field()]
                        .read(cx)
                        .focus_handle(cx),
                );
            }
        }
        cx.notify();
    }

    /// Build the grid on first use, then fold any form edit in flight into the
    /// untouched cells.
    fn seed_cells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(baselines) = self.baselines.clone() else {
            return;
        };
        let created = self.cells.is_none();
        if created {
            let library = self.library.clone();
            let mut cells = Vec::with_capacity(self.tracks.len());
            for _ in &self.tracks {
                let mut row = Vec::with_capacity(FIELDS.len());
                for (field, _, _) in FIELDS {
                    // No PressEnter subscription: enter from a cell reaches the root's binding.
                    let input = cx.new(|cx| {
                        let mut input =
                            InputState::new(window, cx).placeholder(field_placeholder(field));
                        input.lsp.completion_provider = suggest::provider(&library, field, cx);
                        input
                    });
                    self._input_events.push(cx.subscribe_in(
                        &input,
                        window,
                        |_: &mut Self, _, event: &InputEvent, _, cx| {
                            if matches!(event, InputEvent::Change) {
                                cx.notify();
                            }
                        },
                    ));
                    row.push(input);
                }
                cells.push(row);
            }
            // Authored rows stay out until they're on disk.
            let mut tags: Vec<TagColumn> = Vec::new();
            let mut tag_cells: Vec<Vec<TagCell>> = vec![Vec::new(); self.tracks.len()];
            for ix in 0..self.additional.as_ref().map_or(0, |a| a.columns) {
                let Some((key, name, text)) = self
                    .additional
                    .as_ref()
                    .and_then(|a| a.rows.get(ix))
                    .map(|row| (row.key.clone(), row.label.clone(), row.input.is_some()))
                else {
                    continue;
                };
                for (t, row) in tag_cells.iter_mut().enumerate() {
                    row.push(if text {
                        TagCell::Edit(cx.new(|cx| InputState::new(window, cx)))
                    } else {
                        let size = self
                            .additional_baselines
                            .get(t)
                            .and_then(|baseline| baseline.as_ref())
                            .into_iter()
                            .flatten()
                            .find(|(k, _)| *k == key)
                            .map(|(_, value)| one_line(&value.display()))
                            .unwrap_or_default();
                        TagCell::Fixed(size.into())
                    });
                }
                tags.push(TagColumn { key, name, text });
            }
            // Bare disabled inputs, so the names select and copy.
            let names: Vec<Entity<InputState>> = self
                .tracks
                .iter()
                .map(|track| {
                    let name = file_name(&track.path);
                    cx.new(|cx| InputState::new(window, cx).default_value(name))
                })
                .collect();
            let (saved, tag_widths) = Settings::load()
                .windows
                .tag_editor
                .map(|s| (s.columns, s.tag_columns))
                .unwrap_or_default();
            let delegate = CellGrid {
                columns: grid_columns(&saved, &tag_widths, &tags, &self.hidden, &self.shown),
                cells: cells.clone(),
                tags: tags.clone(),
                tag_cells: tag_cells.clone(),
                names,
                order: (0..cells.len()).collect(),
                editor: cx.entity().downgrade(),
            };
            let grid = cx.new(|cx| TableState::new(delegate, window, cx));
            // The component owns the live widths; copy a resize into the delegate so
            // a re-prepare and the close path keep it.
            self._input_events.push(cx.subscribe_in(
                &grid,
                window,
                |_: &mut Self, grid, event: &TableEvent, _, cx| {
                    if let TableEvent::ColumnWidthsChanged(widths) = event {
                        let widths = widths.clone();
                        grid.update(cx, |table, _| {
                            let columns = &mut table.delegate_mut().columns;
                            for (column, width) in columns.iter_mut().zip(widths) {
                                column.width = width;
                            }
                        });
                    }
                },
            ));
            self.grid = Some(grid);
            self.cells = Some(cells);
            self.tag_cells = Some(tag_cells);
            self.seeds = vec![vec![SharedString::default(); self.tracks.len()]; FIELDS.len()];
            self.tag_seeds = vec![vec![SharedString::default(); self.tracks.len()]; tags.len()];
        }
        for (i, (field, _, _)) in FIELDS.iter().enumerate() {
            let form_value = self.inputs[i].read(cx).value().to_string();
            let filled = self.filled[i].clone();
            let bases: Vec<String> = baselines
                .iter()
                .map(|baseline| baseline_value(baseline, field).to_owned())
                .collect();
            let cells: Vec<Entity<InputState>> = self
                .cells
                .iter()
                .flatten()
                .map(|row| row[i].clone())
                .collect();
            let drifted = fold_column(
                &form_value,
                &filled,
                &bases,
                &cells,
                &mut self.seeds[i],
                created,
                window,
                cx,
            );
            if drifted {
                self.filled[i] = form_value.into();
            }
            // The cells hold the truth now; a left-armed clear would wipe every file
            // while the table showed the originals.
            self.cleared[i] = false;
        }
        for ix in 0..self.tag_seeds.len() {
            let Some((key, filled, input)) = self
                .additional
                .as_ref()
                .and_then(|a| a.rows.get(ix))
                .map(|row| (row.key.clone(), row.initial.clone(), row.input.clone()))
            else {
                continue;
            };
            let Some(input) = input else { continue };
            let form_value = input.read(cx).value().to_string();
            let bases: Vec<String> = self
                .additional_baselines
                .iter()
                .map(|baseline| tag_baseline_value(baseline.as_ref(), &key))
                .collect();
            let cells: Vec<Entity<InputState>> = self
                .tag_cells
                .iter()
                .flatten()
                .filter_map(|row| match &row[ix] {
                    TagCell::Edit(cell) => Some(cell.clone()),
                    TagCell::Fixed(_) => None,
                })
                .collect();
            let drifted = fold_column(
                &form_value,
                &filled,
                &bases,
                &cells,
                &mut self.tag_seeds[ix],
                created,
                window,
                cx,
            );
            if drifted && let Some(row) = self.additional.as_mut().and_then(|a| a.rows.get_mut(ix))
            {
                row.initial = form_value.into();
            }
        }
    }

    /// The form re-reads the cells and the fill snapshot follows, so only
    /// typing from here counts as a bulk edit.
    fn refill_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fills: Vec<(SharedString, bool)> = {
            let Some(cells) = &self.cells else {
                return;
            };
            (0..FIELDS.len())
                .map(|i| {
                    let mut values = cells.iter().map(|row| row[i].read(cx).value().clone());
                    let first = values.next().unwrap_or_default();
                    let mixed = values.any(|v| v != first);
                    (
                        if mixed {
                            SharedString::default()
                        } else {
                            first
                        },
                        mixed,
                    )
                })
                .collect()
        };
        for (i, (value, mixed)) in fills.into_iter().enumerate() {
            self.inputs[i].update(cx, |input, cx| {
                let placeholder: SharedString = if mixed {
                    rox_i18n::t!("tags-editor-multiple-values")
                } else {
                    field_placeholder(&FIELDS[i].0).into()
                };
                input.set_placeholder(placeholder, window, cx);
                input.set_value(value.clone(), window, cx);
            });
            self.filled[i] = value;
            self.mixed[i] = mixed;
            self.cleared[i] = false;
        }
        // A tag the table split reads as mixed, not as whichever file came first.
        for ix in 0..self.tag_seeds.len() {
            let cells: Vec<Entity<InputState>> = self
                .tag_cells
                .iter()
                .flatten()
                .filter_map(|row| match &row[ix] {
                    TagCell::Edit(cell) => Some(cell.clone()),
                    TagCell::Fixed(_) => None,
                })
                .collect();
            if cells.is_empty() {
                continue;
            }
            let mut values = cells.iter().map(|cell| cell.read(cx).value().clone());
            let first = values.next().unwrap_or_default();
            let mixed = values.any(|value| value != first);
            let value = if mixed {
                SharedString::default()
            } else {
                first
            };
            let Some(input) = self
                .additional
                .as_ref()
                .and_then(|a| a.rows.get(ix))
                .and_then(|row| row.input.clone())
            else {
                continue;
            };
            input.update(cx, |input, cx| {
                let placeholder: SharedString = if mixed {
                    rox_i18n::t!("tags-editor-multiple-values")
                } else {
                    SharedString::default()
                };
                input.set_placeholder(placeholder, window, cx);
                input.set_value(value.clone(), window, cx);
            });
            if let Some(row) = self.additional.as_mut().and_then(|a| a.rows.get_mut(ix)) {
                row.initial = value;
                row.mixed = mixed;
            }
        }
    }

    /// Only the shared form's mixed fields get this.
    fn toggle_clear(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.cleared.get(i).copied().unwrap_or(false);
        self.cleared[i] = on;
        self.inputs[i].update(cx, |input, cx| {
            if on {
                input.set_value("", window, cx);
                input.set_placeholder(rox_i18n::t!("tags-editor-clear-on-save"), window, cx);
            } else {
                input.set_placeholder(rox_i18n::t!("tags-editor-multiple-values"), window, cx);
            }
        });
        cx.notify();
    }

    /// Remembered: a library either carries romanizations or it doesn't.
    fn toggle_sort_fields(&mut self, cx: &mut Context<Self>) {
        self.sort_fields = !self.sort_fields;
        cx.notify();
    }

    fn toggle_guess(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guess = !self.guess;
        if self.guess {
            window.focus(&self.pattern.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    fn toggle_replace(
        &mut self,
        target: ReplaceTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.replace == Some(target) {
            self.replace = None;
        } else {
            self.replace = Some(target);
            window.focus(&self.find.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    fn close_replace(&mut self, cx: &mut Context<Self>) {
        self.replace = None;
        cx.notify();
    }

    fn toggle_replace_regex(&mut self, cx: &mut Context<Self>) {
        self.replace_regex = !self.replace_regex;
        cx.notify();
    }

    fn toggle_replace_ignore_case(&mut self, cx: &mut Context<Self>) {
        self.replace_ignore_case = !self.replace_ignore_case;
        cx.notify();
    }

    /// The cell once the grid exists, the file's baseline before that.
    fn replace_values(&self, target: ReplaceTarget, cx: &App) -> Vec<String> {
        match target {
            ReplaceTarget::Field(i) => match &self.cells {
                Some(cells) => cells
                    .iter()
                    .map(|row| row[i].read(cx).value().to_string())
                    .collect(),
                None => self
                    .baselines
                    .iter()
                    .flatten()
                    .map(|baseline| baseline_value(baseline, &FIELDS[i].0).to_owned())
                    .collect(),
            },
            ReplaceTarget::Tag(ix) => {
                let Some(key) = self
                    .additional
                    .as_ref()
                    .and_then(|a| a.rows.get(ix))
                    .map(|row| row.key.clone())
                else {
                    return Vec::new();
                };
                match &self.tag_cells {
                    Some(cells) => cells
                        .iter()
                        .map(|row| match row.get(ix) {
                            Some(TagCell::Edit(cell)) => cell.read(cx).value().to_string(),
                            _ => String::new(),
                        })
                        .collect(),
                    None => self
                        .additional_baselines
                        .iter()
                        .map(|baseline| tag_baseline_value(baseline.as_ref(), &key))
                        .collect(),
                }
            }
        }
    }

    fn replace_rule(&self, cx: &App) -> Result<Option<replace::Rule>, String> {
        replace::compile(
            &self.find.read(cx).value(),
            &self.replacement.read(cx).value(),
            self.replace_regex,
            self.replace_ignore_case,
        )
    }

    /// Writes into the cells like the guesser: the values arm like typing, and
    /// the seeds stay put so a replaced value never reseeds away. The form then
    /// re-reads itself from the cells.
    fn apply_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.baselines.is_none() {
            return;
        }
        let Some(target) = self.replace else {
            return;
        };
        let Ok(Some(rule)) = self.replace_rule(cx) else {
            return;
        };
        // Fold any form drift into the cells first, so the rule reads it.
        self.seed_cells(window, cx);
        let changes: Vec<(usize, String)> = self
            .replace_values(target, cx)
            .iter()
            .enumerate()
            .filter_map(|(t, before)| rule.apply(before).map(|after| (t, after)))
            .collect();
        match target {
            ReplaceTarget::Field(i) => {
                let Some(cells) = self.cells.clone() else {
                    return;
                };
                for (t, after) in changes {
                    cells[t][i].update(cx, |cell, cx| cell.set_value(after, window, cx));
                }
            }
            ReplaceTarget::Tag(ix) => {
                let Some(cells) = self.tag_cells.clone() else {
                    return;
                };
                for (t, after) in changes {
                    if let Some(TagCell::Edit(cell)) = cells[t].get(ix) {
                        cell.update(cx, |cell, cx| cell.set_value(after, window, cx));
                    }
                }
            }
        }
        if !self.table {
            self.refill_form(window, cx);
        }
        cx.notify();
    }

    /// Only the values that change list, before and after.
    fn replace_panel(&self, target: ReplaceTarget, cx: &mut Context<Self>) -> Div {
        let values = self.replace_values(target, cx);
        let rule = self.replace_rule(cx);
        let mut rows: Vec<(&str, String)> = Vec::new();
        let error = match &rule {
            Ok(Some(rule)) => {
                for before in &values {
                    if let Some(after) = rule.apply(before) {
                        rows.push((before, after));
                    }
                }
                None
            }
            Ok(None) => None,
            Err(e) => Some(SharedString::from(e.clone())),
        };
        let hits = rows.len();
        let status: SharedString = match error {
            Some(e) => e,
            None => rox_i18n::t!(
                "tags-editor-replace-match-count",
                hits = hits as u64,
                total = values.len() as u64
            ),
        };
        let preview = rows
            .into_iter()
            .map(|(before, after)| {
                let (after, color) = if after.is_empty() {
                    ("-".to_owned(), palette::text_faint())
                } else {
                    (after, palette::text_bright())
                };
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(tokens::SPACE_MD)
                    .py(px(1.))
                    .text_xs()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(before.to_owned())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette::text_faint())
                            .child("→"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(color)
                            .child(SharedString::from(after)),
                    )
            })
            .collect::<Vec<_>>();
        let inert = self.saving || self.baselines.is_none();
        let label = |text: SharedString| {
            div()
                .w(px(84.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(text)
        };
        // Enter applies the rule and stops short of the root's save, which would
        // close the window under the preview.
        let boxed = |input: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .on_action(|_: &Save, _, cx: &mut App| cx.stop_propagation())
                .child(Input::new(input).small())
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .my(tokens::SPACE_XS)
            .border_1()
            .border_color(palette::border())
            .rounded(tokens::RADIUS)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(label(rox_i18n::t!("tags-editor-replace-find")))
                    .child(boxed(&self.find))
                    .child(switch_row(
                        "replace-regex",
                        self.replace_regex,
                        rox_i18n::t!("tags-editor-replace-regex"),
                        cx.listener(|this, _, _, cx| this.toggle_replace_regex(cx)),
                    ))
                    .child(switch_row(
                        "replace-ignore-case",
                        self.replace_ignore_case,
                        rox_i18n::t!("tags-editor-replace-ignore-case"),
                        cx.listener(|this, _, _, cx| this.toggle_replace_ignore_case(cx)),
                    ))
                    .child(
                        div()
                            .id("replace-close")
                            .flex_none()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.close_replace(cx)))
                            .child(
                                svg()
                                    .path(icons::CLOSE)
                                    .size(px(12.))
                                    .text_color(palette::text_muted()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(label(rox_i18n::t!("tags-editor-replace-with")))
                    .child(boxed(&self.replacement))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("tags-editor-replace-apply"),
                        icons::ARROW_DOWN,
                        inert || hits == 0,
                        cx.listener(|this, _, window, cx| this.apply_replace(window, cx)),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("tags-editor-replace-help")),
            )
            .when(hits > 0, |d| {
                d.child(
                    div()
                        .id("replace-preview")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(preview),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(status),
            )
    }

    /// Per-track values go into the table's cells, switching to table mode; a
    /// single track on the form fills its fields.
    fn apply_guesses(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.baselines.is_none() {
            return;
        }
        let Ok(pattern) = guess::parse(self.pattern.read(cx).value().trim()) else {
            return;
        };
        let matches: Vec<Option<Vec<(Field, String)>>> = self
            .tracks
            .iter()
            .map(|track| pattern.apply(&track.path))
            .collect();
        if matches.iter().all(|matched| matched.is_none()) {
            return;
        }
        if self.tracks.len() == 1 && !self.table {
            if let Some(values) = &matches[0] {
                for (field, value) in values {
                    let Some(i) = FIELDS.iter().position(|(f, _, _)| f == field) else {
                        continue;
                    };
                    let value = value.clone();
                    self.inputs[i].update(cx, |input, cx| input.set_value(value, window, cx));
                }
            }
        } else {
            if !self.table {
                self.table = true;
            }
            self.seed_cells(window, cx);
            let Some(cells) = self.cells.clone() else {
                return;
            };
            // The seeds stay put, so a guessed value never reseeds away.
            for (t, matched) in matches.iter().enumerate() {
                let Some(values) = matched else {
                    continue;
                };
                for (field, value) in values {
                    let Some(i) = FIELDS.iter().position(|(f, _, _)| f == field) else {
                        continue;
                    };
                    let value = value.clone();
                    cells[t][i].update(cx, |cell, cx| cell.set_value(value, window, cx));
                }
            }
        }
        cx.notify();
    }

    fn guess_panel(&self, cx: &mut Context<Self>) -> Div {
        const PREVIEW_CAP: usize = 8;
        let parsed = guess::parse(self.pattern.read(cx).value().trim());
        let (matches, parse_error) = match &parsed {
            Ok(pattern) => (
                self.tracks
                    .iter()
                    .map(|track| pattern.apply(&track.path))
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(e) => (Vec::new(), Some(SharedString::from(e.clone()))),
        };
        let hits = matches.iter().flatten().count();
        let rows = self
            .tracks
            .iter()
            .zip(&matches)
            .take(PREVIEW_CAP)
            .map(|(track, matched)| {
                let name = track
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| track.path.display().to_string());
                let values: gpui::AnyElement = match matched {
                    Some(values) => div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap_x(tokens::SPACE_MD)
                        .children(values.iter().map(|(field, value)| {
                            let label = FIELDS
                                .iter()
                                .find(|(f, _, _)| f == field)
                                .map(|(_, label, _)| *label)
                                .unwrap_or("field");
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_color(palette::text_muted())
                                        .child(SharedString::from(title_case(label))),
                                )
                                .child(SharedString::from(value.clone()))
                        }))
                        .into_any_element(),
                    None => div()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("tags-editor-guess-no-match"))
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(tokens::SPACE_MD)
                    .text_xs()
                    .child(
                        div()
                            .flex_none()
                            .w(px(280.))
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(name)),
                    )
                    .child(div().flex_1().min_w_0().child(values))
            })
            .collect::<Vec<_>>();
        let folded = if parse_error.is_none() {
            self.tracks.len().saturating_sub(PREVIEW_CAP)
        } else {
            0
        };
        let status: SharedString = match parse_error {
            Some(e) => e,
            None => rox_i18n::t!(
                "tags-editor-guess-match-count",
                hits = hits as u64,
                total = self.tracks.len() as u64
            ),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .mb(tokens::SPACE_XS)
            .border_1()
            .border_color(palette::border())
            .rounded(tokens::RADIUS)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .w(px(84.))
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("tags-editor-guess-pattern-label")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            // Stops short of the root's save, which would close the window.
                            .on_action(|_: &Save, _, cx: &mut App| cx.stop_propagation())
                            .child(panel::pattern_input(
                                "guess-pattern",
                                &self.pattern,
                                guess::PLACEHOLDERS,
                                vec![rox_i18n::t!("tags-editor-guess-help")],
                                None,
                            )),
                    )
                    .child(settings_ui::small_button(
                        "Apply",
                        icons::ARROW_DOWN,
                        self.saving || self.baselines.is_none() || hits == 0,
                        cx.listener(|this, _, window, cx| this.apply_guesses(window, cx)),
                    )),
            )
            .children(rows)
            .child(div().text_xs().text_color(palette::text_muted()).map(|d| {
                if folded > 0 {
                    d.child(rox_i18n::t!(
                        "tags-editor-guess-folded",
                        status = status.to_string(),
                        count = folded as u64
                    ))
                } else {
                    d.child(status)
                }
            }))
    }

    /// The compare calls back into [`Self::fill_fields`] rather than writing,
    /// so this editor stays the one writer.
    fn look_up(&mut self, track: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.tracks.get(track) else {
            return;
        };
        let key = TrackKey {
            source: local(),
            path: row.path.clone(),
            sub: row.sub,
        };
        let library = self.library.clone();
        let now_art = self.now_art.clone();
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        crate::tags::matcher::open_fill(library, now_art, key, track, weak, handle, cx);
    }

    /// Each set input arms as a pending edit; nothing reaches disk until save.
    /// The values go to the named track's cells once the grid is up, and to the
    /// shared form only for a single track, since a batch form would stamp one
    /// release over every file.
    pub fn fill_fields(
        &mut self,
        track: usize,
        values: &[(Field, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to_cells = self.cells.is_some() && (self.table || self.tracks.len() > 1);
        for (field, value) in values {
            let Some(i) = FIELDS.iter().position(|(f, _, _)| f == field) else {
                continue;
            };
            let value = value.clone();
            match to_cells {
                true => {
                    let Some(cell) = self
                        .cells
                        .as_ref()
                        .and_then(|cells| cells.get(track))
                        .map(|row| row[i].clone())
                    else {
                        continue;
                    };
                    cell.update(cx, |cell, cx| cell.set_value(value, window, cx));
                }
                false => self.inputs[i].update(cx, |input, cx| input.set_value(value, window, cx)),
            }
        }
        if fills_sort_field(values) {
            self.sort_fields = true;
        }
        if to_cells {
            for label in sort_columns_to_show(values, &self.shown) {
                self.toggle_column(label.into(), cx);
            }
        }
        cx.notify();
    }

    /// Diffed per file against its own baseline, so unchanged fields never
    /// rewrite. A failure keeps the form open, the failed files untouched.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baselines), false, false) = (&self.baselines, self.saving, self.saved) else {
            return;
        };
        let single = self.tracks.len() == 1;
        let mut armed: Vec<(usize, String)> = Vec::new();
        for (i, (_, _, per_track)) in FIELDS.iter().enumerate() {
            // Skipped in a batch, so a stale fill never counts as an edit.
            if *per_track && !single {
                continue;
            }
            let value = self.inputs[i].read(cx).value().to_string();
            // An armed clear counts even when the input matches its fill.
            if value == self.filled[i].as_ref() && !self.cleared[i] {
                continue;
            }
            armed.push((i, value));
        }
        let mut edits = Vec::new();
        for (t, (track, baseline)) in self.tracks.iter().zip(baselines).enumerate() {
            let mut changes = Vec::new();
            for (i, (field, _, _)) in FIELDS.iter().enumerate() {
                // A form edit is the newest typing and wins; otherwise the cell supplies
                // the value.
                let value = match armed.iter().find(|(armed_ix, _)| *armed_ix == i) {
                    Some((_, value)) => value.clone(),
                    None => match &self.cells {
                        Some(cells) => cells[t][i].read(cx).value().to_string(),
                        None => continue,
                    },
                };
                if let Some(change) = change_for(field, value, baseline) {
                    changes.push(change);
                }
            }
            // Same rule for the additional rows. A file whose read failed stays
            // untouched.
            if let (Some(additional), Some(Some(rows))) =
                (&self.additional, self.additional_baselines.get(t))
            {
                let mut intents: Vec<(String, TagIntent)> = Vec::new();
                for (ix, row) in additional.rows.iter().enumerate() {
                    let Some(key) = row.key(cx) else { continue };
                    let armed = match (row.removed, &row.input) {
                        (true, _) => Some(TagIntent::Drop),
                        (false, Some(input)) => {
                            let value = input.read(cx).value().to_string();
                            (value != row.initial.as_ref()).then_some(TagIntent::Set(value))
                        }
                        (false, None) => None,
                    };
                    let intent = match armed {
                        Some(intent) => intent,
                        None => match self
                            .tag_cells
                            .as_ref()
                            .and_then(|cells| cells.get(t))
                            .and_then(|row| row.get(ix))
                        {
                            Some(TagCell::Edit(cell)) => {
                                TagIntent::Set(cell.read(cx).value().to_string())
                            }
                            Some(TagCell::Fixed(_)) | None => TagIntent::Keep,
                        },
                    };
                    intents.push((key, intent));
                }
                for (key, intent) in fold_tag_intents(intents) {
                    if let Some(change) = tag_change_for(&key, &intent, rows) {
                        changes.push(change);
                    }
                }
            }
            if !changes.is_empty() {
                // A writer::Edit names a file, and one file can be a dozen cue tracks.
                edits.push((
                    Edit {
                        path: track.path.clone(),
                        changes,
                        pictures: Vec::new(),
                    },
                    track.sub,
                ));
            }
        }
        if edits.is_empty() {
            self.saved = true;
            self.persist_frame(window, cx);
            window.remove_window();
            return;
        }
        self.saving = true;
        self.save_done = 0;
        self.save_total = edits.len();
        self.error = None;
        cx.notify();
        let library = self.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            // Note the whole batch up front: it lands well inside the suppression
            // window, and a per-file note would put a main-thread round trip before
            // every commit.
            if library
                .update(cx, |library, _| {
                    library.note_self_write(edits.iter().map(|(edit, _)| edit.path.clone()))
                })
                .is_err()
            {
                return;
            }
            // A small pool over a shared queue, so a batch doesn't cost the sum of its
            // files and a slow one holds up only its own worker. Capped like the other
            // pools, since this is one drive.
            let total = edits.len();
            let queue = Arc::new(Mutex::new(
                edits
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (edit, sub))| (ix, edit, sub))
                    .collect::<VecDeque<_>>(),
            ));
            let (tx, rx) = async_channel::unbounded();
            let count = std::thread::available_parallelism()
                .map(|n| n.get() / 2)
                .unwrap_or(1)
                .clamp(1, SAVE_WORKERS)
                .min(total);
            let workers: Vec<_> = (0..count)
                .map(|_| {
                    let (queue, tx) = (queue.clone(), tx.clone());
                    cx.background_executor().spawn(async move {
                        loop {
                            let next = queue.lock().unwrap().pop_front();
                            let Some((ix, edit, sub)) = next else {
                                break;
                            };
                            // Through the key, so a cue track's edit stays in the library.
                            let result =
                                writer::commit_key(&edit.path, sub, &edit.changes, &edit.pictures);
                            if tx.send((ix, edit, sub, result)).await.is_err() {
                                break;
                            }
                        }
                    })
                })
                .collect();
            // Without this the recv never sees the queue run dry.
            drop(tx);
            let mut committed: Vec<Edit> = Vec::new();
            let mut committed_subs: Vec<u16> = Vec::new();
            let mut failures = 0usize;
            // Name the first file in list order, not the first worker to fail.
            let mut first_error: Option<(usize, String)> = None;
            while let Ok((ix, edit, sub, result)) = rx.recv().await {
                match result {
                    Ok(()) => {
                        committed.push(edit);
                        committed_subs.push(sub);
                    }
                    Err(e) => {
                        failures += 1;
                        if first_error.as_ref().is_none_or(|(at, _)| ix < *at) {
                            let name = edit
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| edit.path.display().to_string());
                            first_error = Some((ix, format!("{name}: {e}")));
                        }
                    }
                }
                // A closed window drops the workers; commits already running finish.
                if this
                    .update(cx, |this, cx| {
                        this.save_done += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            drop(workers);
            let first_error = first_error.map(|(_, e)| e);
            this.update_in(cx, move |this, window, cx| {
                // A retry after a partial failure diffs against the new baselines.
                for edit in &committed {
                    let Some(ix) = this.tracks.iter().position(|t| t.path == edit.path) else {
                        continue;
                    };
                    for change in &edit.changes {
                        // A set replaced every carrier of the key.
                        if let Field::Unknown(key) = &change.field {
                            let Some(Some(rows)) = this.additional_baselines.get_mut(ix) else {
                                continue;
                            };
                            rows.retain(|(k, _)| k != key);
                            if let Some(value) = &change.value {
                                rows.push((key.clone(), UnknownValue::Text(value.clone())));
                            }
                            continue;
                        }
                        let Some(baseline) = this.baselines.as_mut().and_then(|b| b.get_mut(ix))
                        else {
                            continue;
                        };
                        match &change.value {
                            Some(value) => {
                                match baseline.iter_mut().find(|(f, _)| f == &change.field) {
                                    Some(entry) => entry.1 = value.clone(),
                                    None => baseline.push((change.field.clone(), value.clone())),
                                }
                            }
                            None => baseline.retain(|(f, _)| f != &change.field),
                        }
                    }
                }
                if !committed.is_empty() {
                    library.update(cx, |library, cx| {
                        library.apply_edits(&committed, &committed_subs, cx)
                    });
                }
                match first_error {
                    None => {
                        this.persist_frame(window, cx);
                        window.remove_window();
                    }
                    Some(e) => {
                        this.saving = false;
                        this.error = Some(if failures > 1 {
                            rox_i18n::t!("tags-editor-save-errors", count = failures, error = e)
                        } else {
                            e.into()
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The tags no field addresses, under their own fold. Hand-rolled header
    /// rather than [`section`], whose label is static. Drawn even when empty,
    /// since the add button lives in its header.
    fn additional_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let additional = self.additional.as_ref()?;
        let open = self.additional_open;
        let mut body = div().flex().flex_col();
        if additional.failed > 0 {
            body = body.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "tags-editor-unread-count",
                        failed = additional.failed as u64,
                        total = additional.files as u64
                    )),
            );
        }
        for (i, row) in additional.rows.iter().enumerate() {
            let removed = row.removed;
            let value: gpui::AnyElement = match (&row.input, &row.binary) {
                (Some(input), _) => Input::new(input)
                    .small()
                    .appearance(false)
                    .disabled(self.saving || removed)
                    .into_any_element(),
                (None, Some(size)) => div()
                    .truncate()
                    .text_color(palette::text_muted())
                    .child(size.clone())
                    .into_any_element(),
                (None, None) => div().into_any_element(),
            };
            let (key, conflict): (gpui::AnyElement, Option<SharedString>) = match &row.key_input {
                Some(input) => {
                    let typed = input.read(cx).value().to_string();
                    let conflict = field_owning(&typed)
                        .map(|field| rox_i18n::t!("tags-editor-tag-field-conflict", field = field));
                    (
                        Input::new(input)
                            .small()
                            .appearance(false)
                            .disabled(self.saving || removed)
                            .into_any_element(),
                        conflict,
                    )
                }
                None => (
                    div()
                        .truncate()
                        .text_color(palette::text_muted())
                        .when(removed, |d| d.line_through())
                        .child(row.label.clone())
                        .into_any_element(),
                    // A file tag whose name folds to a field's label is edited under its own
                    // key; the note makes the collision visible.
                    field_owning(&row.key)
                        .map(|field| rox_i18n::t!("tags-editor-tag-field-conflict", field = field)),
                ),
            };
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .py(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(div().w(px(180.)).flex_none().min_w_0().child(key))
                    .child(div().flex_1().min_w_0().child(value))
                    // An authored key a field owns saves nothing; a row off a file still saves
                    // under its own key.
                    .when_some(conflict, |d, note| {
                        d.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::tone_warn())
                                .child(note),
                        )
                    })
                    .when(row.files < additional.files, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!(
                                    "tags-editor-unknown-partial",
                                    count = row.files as u64,
                                    total = additional.files as u64
                                )),
                        )
                    })
                    .child(
                        div()
                            .id(("remove-tag", i))
                            .flex_none()
                            .px(tokens::SPACE_XS)
                            .py(px(1.))
                            .rounded(tokens::RADIUS)
                            .text_xs()
                            .cursor_pointer()
                            .map(|d| {
                                if removed {
                                    d.text_color(palette::accent())
                                } else {
                                    d.text_color(palette::text_muted())
                                        .hover(|d| d.text_color(palette::text()))
                                }
                            })
                            .child(if removed {
                                rox_i18n::t!("tags-editor-will-remove")
                            } else {
                                rox_i18n::t!("tags-editor-remove")
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_remove_additional(i, cx)
                            })),
                    )
                    .when(row.mixed && row.input.is_some() && !removed, |d| {
                        d.child(arm_chip(
                            ("replace-tag", i),
                            self.replace == Some(ReplaceTarget::Tag(i)),
                            rox_i18n::t!("tags-editor-replace"),
                            cx.listener(move |this, _, window, cx| {
                                this.toggle_replace(ReplaceTarget::Tag(i), window, cx)
                            }),
                        ))
                    }),
            );
            if self.replace == Some(ReplaceTarget::Tag(i)) {
                body = body.child(self.replace_panel(ReplaceTarget::Tag(i), cx));
            }
        }
        // Under the table the page doesn't scroll, so the list scrolls itself.
        let body: gpui::AnyElement = if self.table {
            div()
                .id("additional-rows")
                .max_h(px(240.))
                .overflow_y_scroll()
                .child(body)
                .into_any_element()
        } else {
            body.into_any_element()
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_SM)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(tokens::SPACE_SM)
                        .pb(tokens::SPACE_XS)
                        .border_b_1()
                        .border_color(palette::border())
                        // The fold's hit area stops at the label, so the add button doesn't close
                        // the list.
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(tokens::SPACE_XS)
                                .text_xs()
                                .text_color(palette::text_muted())
                                .cursor_pointer()
                                .hover(|d| d.text_color(palette::text()))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.toggle_additional(cx)),
                                )
                                .child(
                                    svg()
                                        .path(if open {
                                            icons::CHEVRON_DOWN
                                        } else {
                                            icons::CHEVRON_RIGHT
                                        })
                                        .size(px(12.))
                                        .flex_none()
                                        .text_color(palette::text_muted()),
                                )
                                .child(rox_i18n::t!(
                                    "tags-editor-additional-tags",
                                    count = additional.rows.len() as u64
                                )),
                        )
                        .child(settings_ui::small_button(
                            rox_i18n::t!("tags-editor-add-tag"),
                            icons::PLUS,
                            self.saving,
                            cx.listener(|this, _, window, cx| this.add_tag_row(window, cx)),
                        )),
                )
                .when(open, |d| d.child(body)),
        )
    }

    fn tags_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        // Single track only: the compare matches one track's tags, and in the
        // table each row has its own button.
        let single = self.tracks.len() == 1;
        let look_up = (!self.table && single && providers::metadata_online()).then(|| {
            settings_ui::small_button(
                rox_i18n::t!("tags-editor-look-up"),
                icons::DOWNLOAD,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.look_up(0, window, cx)),
            )
            .into_any_element()
        });
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(!self.table, |d| {
                let on = self.sort_fields;
                d.child(
                    div()
                        .id("tag-editor-sort-fields")
                        .flex()
                        .flex_row()
                        .flex_none()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.toggle_sort_fields(cx)),
                        )
                        .child(settings_ui::checkbox(on))
                        .child(
                            div()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("tags-editor-sort-names")),
                        ),
                )
            })
            .when(single, |d| {
                let path = self.tracks[0].path.clone();
                d.child(settings_ui::small_button(
                    rox_i18n::t!("tags-editor-reveal"),
                    icons::FOLDER,
                    false,
                    move |_, _, cx| cx.reveal_path(&path),
                ))
            })
            .when(!single, |d| {
                d.child(settings_ui::small_button(
                    if self.table {
                        rox_i18n::t!("tags-editor-form-view")
                    } else {
                        rox_i18n::t!("tags-editor-table-view")
                    },
                    icons::ROWS_3,
                    self.saving || self.baselines.is_none(),
                    cx.listener(|this, _, window, cx| this.toggle_table(window, cx)),
                ))
            })
            .child(settings_ui::small_button(
                rox_i18n::t!("tags-editor-guess-button"),
                icons::FILE_TEXT,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.toggle_guess(window, cx)),
            ))
            .into_any_element();
        let body = if self.table {
            self.table_body()
        } else {
            self.form_body(cx).into_any_element()
        };
        // Lock the fields while a commit is in flight. Cancel is outside it.
        let content = div()
            .relative()
            .flex()
            .flex_col()
            .when(self.table, |d| d.flex_1().min_h_0())
            .when(self.guess, |d| d.child(self.guess_panel(cx)))
            .child(body)
            .when(self.saving, |d| {
                d.child(div().absolute().inset_0().occlude())
            });
        match look_up {
            Some(control) => section_with_control(
                rox_i18n::t!("tags-editor-tags-section"),
                control,
                Some(buttons),
                content,
            ),
            None => section(
                rox_i18n::t!("tags-editor-tags-section"),
                Some(buttons),
                content,
            ),
        }
    }

    fn savable(&self) -> bool {
        !self.saving && self.baselines.is_some()
    }

    /// On the root rather than either page, so the buttons hold still when the
    /// form and table swap.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let hint: gpui::AnyElement = if self.saving {
            let label = {
                let at = (self.save_done + 1).min(self.save_total);
                rox_i18n::t!(
                    "tags-editor-saving-progress",
                    done = at as u64,
                    total = self.save_total as u64
                )
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::tone_warn())
                .child(Spinner::new().with_size(Size::Small))
                .child(label)
                .into_any_element()
        } else {
            // An unsupported format isn't a broken file, so it gets its own line.
            let reason: Option<SharedString> = if self.unsupported > 0 {
                Some(if self.unsupported == self.tracks.len() {
                    rox_i18n::t!("tags-editor-format-unsupported-all")
                } else {
                    rox_i18n::t!("tags-editor-format-unsupported-some")
                })
            } else if self.error.is_some() {
                self.error.clone()
            } else if self.baselines.is_none() {
                Some(rox_i18n::t!("tags-editor-loading"))
            } else {
                None
            };
            match reason {
                Some(reason) => div()
                    .text_xs()
                    .text_color(palette::tone_warn())
                    .child(reason)
                    .into_any_element(),
                None => kbd_line([
                    Seg::Text("Press".into()),
                    Seg::Key("Enter".into()),
                    Seg::Text("to save".into()),
                ])
                .text_xs()
                .into_any_element(),
            }
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(hint)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        "Save",
                        icons::CHECK,
                        !self.savable(),
                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                    ))
                    // Cancel stays live through a save: the atomic writer leaves every
                    // original intact either way.
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|this, _, window, cx| {
                            this.persist_frame(window, cx);
                            window.remove_window();
                        }),
                    )),
            )
    }

    /// Per-track fields read as plain text in a batch; the table edits them.
    fn form_body(&self, cx: &mut Context<Self>) -> Div {
        let single = self.tracks.len() == 1;
        let shown = form_fields(self.sort_fields);
        let label_w = px(label_column_w(&shown));
        let rows = shown.iter().copied().map(|i| {
            let (field_def, label, per_track) = &FIELDS[i];
            let target = ReplaceTarget::Field(i);
            let mixed = self.mixed.get(i).copied().unwrap_or(false);
            // Typing can only add a value to a mixed field, never clear it
            // everywhere. The toggle does that.
            let clearable = !single && !per_track && mixed;
            let cleared = self.cleared.get(i).copied().unwrap_or(false);
            // Per-track fields too: a batch of titles with a junk suffix is the case
            // the panel is for.
            let replaceable = !single && mixed;
            let replacing = self.replace == Some(target);
            let field: gpui::AnyElement = if *per_track && !single {
                let value = self.inputs[i].read(cx).value();
                let (text, faded) = if mixed {
                    (rox_i18n::t!("tags-editor-multiple-values"), true)
                } else if value.is_empty() {
                    (SharedString::from("-"), true)
                } else {
                    (value, false)
                };
                div()
                    .when(faded, |d| d.text_color(palette::text_muted()))
                    .when(mixed, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.text_color(palette::text()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.toggle_replace(ReplaceTarget::Field(i), window, cx)
                                }),
                            )
                    })
                    .child(text)
                    .into_any_element()
            } else if *field_def == Field::Rating && rating_style() == RatingStyle::Stars {
                rating_field(&self.inputs[i], cx).into_any_element()
            } else {
                // Tab takes the open suggestion; the move itself is the stock next stop.
                let input = self.inputs[i].clone();
                div()
                    .key_context("TagField")
                    .on_action({
                        let input = input.clone();
                        move |_: &FieldTab, window, cx| {
                            take_suggestion(&input, window, cx);
                            window.focus_next();
                            // Or the root's tab binding moves focus a second time.
                            cx.stop_propagation();
                        }
                    })
                    .on_action(move |_: &FieldTabPrev, window, cx| {
                        take_suggestion(&input, window, cx);
                        window.focus_prev();
                        cx.stop_propagation();
                    })
                    .child(Input::new(&self.inputs[i]).small().disabled(self.saving))
                    .into_any_element()
            };
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .h(px(26.))
                .child(
                    div()
                        .w(label_w)
                        .flex_none()
                        .text_color(palette::text_muted())
                        // FIELDS keeps lowercase literals, which the column sets and tests match
                        // on.
                        .child(SharedString::from(title_case(label))),
                )
                .child(div().flex_1().min_w_0().child(field))
                .when(clearable, |d| {
                    d.child(
                        div()
                            .id(("clear-field", i))
                            .flex_none()
                            .px(tokens::SPACE_XS)
                            .py(px(1.))
                            .rounded(tokens::RADIUS)
                            .text_xs()
                            .cursor_pointer()
                            .map(|d| {
                                if cleared {
                                    d.text_color(palette::accent())
                                } else {
                                    d.text_color(palette::text_muted())
                                        .hover(|d| d.text_color(palette::text()))
                                }
                            })
                            .child(if cleared {
                                rox_i18n::t!("tags-editor-will-clear")
                            } else {
                                rox_i18n::t!("tags-editor-clear-all")
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_clear(i, window, cx)
                            })),
                    )
                })
                .when(replaceable, |d| {
                    d.child(arm_chip(
                        ("replace-field", i),
                        replacing,
                        rox_i18n::t!("tags-editor-replace"),
                        cx.listener(move |this, _, window, cx| {
                            this.toggle_replace(ReplaceTarget::Field(i), window, cx)
                        }),
                    ))
                });
            if replacing {
                div()
                    .flex()
                    .flex_col()
                    .child(row)
                    .child(self.replace_panel(target, cx))
                    .into_any_element()
            } else {
                row.into_any_element()
            }
        });
        div().flex().flex_col().gap(px(2.)).children(rows)
    }

    fn table_body(&self) -> gpui::AnyElement {
        let Some(grid) = &self.grid else {
            return div().into_any_element();
        };
        div()
            .flex_1()
            .min_h_0()
            .child(Table::new(grid).stripe(true).bordered(true))
            .into_any_element()
    }
}

/// The cells are the editor's own inputs, so the table shows exactly what
/// save reads. `order` maps display rows to track indices.
struct CellGrid {
    columns: Vec<Column>,
    cells: Vec<Vec<Entity<InputState>>>,
    /// A tag column's place here is also its index in `tag_cells`.
    tags: Vec<TagColumn>,
    tag_cells: Vec<Vec<TagCell>>,
    names: Vec<Entity<InputState>>,
    order: Vec<usize>,
    editor: WeakEntity<TagEditor>,
}

/// `saved` widths are positional and go through [`placed_widths`];
/// `tag_widths` is keyed, since the tag set changes with the selection.
/// Hidden columns drop out after the widths resolve, and a set that would
/// empty the table is ignored.
fn grid_columns(
    saved: &[f32],
    tag_widths: &BTreeMap<String, f32>,
    tags: &[TagColumn],
    hidden: &HashSet<String>,
    shown: &HashSet<String>,
) -> Vec<Column> {
    let defaults = default_widths();
    let placed = placed_widths(saved);
    let saved: &[f32] = placed.as_deref().unwrap_or(&[]);
    let width = |i: usize| {
        saved
            .get(i)
            .copied()
            .filter(|w| *w >= 24.)
            .unwrap_or(defaults[i])
    };
    let fixed = column_keys().enumerate().map(|(i, key)| {
        Column::new(key, title_case(key))
            .width(px(width(i)))
            .sortable()
    });
    let tags = tags.iter().map(|tag| {
        let width = tag_widths
            .get(&tag.key)
            .copied()
            .filter(|w| *w >= 24.)
            .unwrap_or(TAG_WIDTH);
        Column::new(format!("{TAG_PREFIX}{}", tag.key), tag.name.clone())
            .width(px(width))
            .sortable()
    });
    let columns: Vec<Column> = fixed.chain(tags).collect();
    let picked: Vec<Column> = columns
        .iter()
        .filter(|column| column_shown(column.key.as_ref(), hidden, shown))
        .cloned()
        .collect();
    if picked.is_empty() {
        // Only the fixed columns, not a wall of every tag.
        columns.into_iter().take(LEAD + FIELDS.len()).collect()
    } else {
        picked
    }
}

impl CellGrid {
    /// By key: hidden columns leave the display order sparse.
    fn kind(&self, col_ix: usize) -> Option<ColumnKind> {
        column_kind(self.columns[col_ix].key.as_ref(), &self.tags)
    }

    /// Hidden columns keep their cells but aren't focused; the file column,
    /// star ratings and binary tags hold no input.
    fn tab_stops(&self, stars: bool) -> Vec<ColumnKind> {
        (0..self.columns.len())
            .filter_map(|ix| self.kind(ix))
            .filter(|kind| match kind {
                ColumnKind::File => false,
                ColumnKind::Field(i) => !(stars && FIELDS[*i].0 == Field::Rating),
                ColumnKind::Tag(ix) => self.tags[*ix].text,
            })
            .collect()
    }

    fn cell(&self, kind: ColumnKind, track: usize) -> Option<Entity<InputState>> {
        match kind {
            ColumnKind::File => None,
            ColumnKind::Field(i) => Some(self.cells[track][i].clone()),
            ColumnKind::Tag(ix) => match &self.tag_cells[track][ix] {
                TagCell::Edit(cell) => Some(cell.clone()),
                TagCell::Fixed(_) => None,
            },
        }
    }

    /// A bare disabled input, which still selects and copies. The lookup lives
    /// on the row since it matches one file.
    fn file_cell(&self, track: usize) -> Div {
        let reveal = self.editor.clone();
        let look_up = self.editor.clone();
        div()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.names[track])
                        .small()
                        .appearance(false)
                        .disabled(true),
                ),
            )
            .child(settings_ui::icon_button(
                icons::FOLDER,
                false,
                move |_, _, cx| {
                    if let Some(editor) = reveal.upgrade() {
                        let path = editor.read(cx).tracks[track].path.clone();
                        cx.reveal_path(&path);
                    }
                },
            ))
            .when(providers::metadata_online(), |d| {
                d.child(settings_ui::icon_button(
                    icons::DOWNLOAD,
                    false,
                    move |_, window, cx| {
                        look_up
                            .update(cx, |editor, cx| editor.look_up(track, window, cx))
                            .ok();
                    },
                ))
            })
    }
}

impl TableDelegate for CellGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let shown: HashSet<String> = self.columns.iter().map(|c| c.key.to_string()).collect();
        let editor = self.editor.clone();
        let tags: Vec<(SharedString, SharedString)> = self
            .tags
            .iter()
            .map(|tag| {
                (
                    SharedString::from(format!("{TAG_PREFIX}{}", tag.key)),
                    tag.name.clone(),
                )
            })
            .collect();
        div()
            .size_full()
            .child(self.column(col_ix, cx).name.clone())
            .context_menu(move |mut menu, _, _| {
                for key in column_keys() {
                    let editor = editor.clone();
                    menu = menu.item(
                        PopupMenuItem::new(title_case(key))
                            .checked(shown.contains(key))
                            .on_click(move |_, _, cx| {
                                editor
                                    .update(cx, |editor, cx| editor.toggle_column(key.into(), cx))
                                    .ok();
                            }),
                    );
                }
                if !tags.is_empty() {
                    menu = menu.separator().item(PopupMenuItem::label(rox_i18n::t!(
                        "tags-editor-tag-columns"
                    )));
                }
                for (key, name) in &tags {
                    let editor = editor.clone();
                    let key = key.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(shown.contains(key.as_ref()))
                            .on_click(move |_, _, cx| {
                                let key = key.clone();
                                editor
                                    .update(cx, |editor, cx| editor.toggle_column(key, cx))
                                    .ok();
                            }),
                    );
                }
                menu
            })
    }

    /// Numerics sort by leading digits, the scanner's read. Cells travel with
    /// their track.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        for (ix, column) in self.columns.iter_mut().enumerate() {
            column.sort = Some(if ix == col_ix {
                sort
            } else {
                ColumnSort::Default
            });
        }
        if matches!(sort, ColumnSort::Default) {
            self.order = (0..self.cells.len()).collect();
            return;
        }
        let kind = self.kind(col_ix);
        let numeric = matches!(kind, Some(ColumnKind::Field(i)) if matches!(
            FIELDS[i].0,
            Field::Year | Field::TrackNo | Field::DiscNo | Field::Rating
        ));
        let mut keyed: Vec<(usize, String)> = self
            .order
            .iter()
            .map(|&t| {
                let value = match kind {
                    Some(ColumnKind::Field(i)) => self.cells[t][i].read(cx).value().to_lowercase(),
                    Some(ColumnKind::Tag(ix)) => match &self.tag_cells[t][ix] {
                        TagCell::Edit(cell) => cell.read(cx).value().to_lowercase(),
                        TagCell::Fixed(size) => size.to_lowercase(),
                    },
                    Some(ColumnKind::File) | None => self.names[t].read(cx).value().to_lowercase(),
                };
                (t, value)
            })
            .collect();
        if numeric {
            keyed.sort_by_key(|(_, value)| leading_number(value));
        } else {
            keyed.sort_by(|a, b| a.1.cmp(&b.1));
        }
        if matches!(sort, ColumnSort::Descending) {
            keyed.reverse();
        }
        self.order = keyed.into_iter().map(|(t, _)| t).collect();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let rows = self.order.len();
        let track = self.order[row_ix];
        let stars = rating_style() == RatingStyle::Stars;
        let kind = self.kind(col_ix);
        match kind {
            // A hand-edited settings key that names nothing draws empty.
            Some(ColumnKind::File) => return self.file_cell(track).into_any_element(),
            None => return div().into_any_element(),
            Some(ColumnKind::Field(i)) if stars && FIELDS[i].0 == Field::Rating => {
                return div()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(rating_field(&self.cells[track][i], cx))
                    .into_any_element();
            }
            Some(ColumnKind::Tag(ix)) => {
                if let TagCell::Fixed(size) = &self.tag_cells[track][ix] {
                    return div()
                        .h_full()
                        .flex()
                        .items_center()
                        .truncate()
                        .text_color(palette::text_muted())
                        .child(size.clone())
                        .into_any_element();
                }
            }
            _ => {}
        }
        let kind = kind.expect("the empty kinds returned above");
        let Some(cell) = self.cell(kind, track) else {
            return div().into_any_element();
        };
        // Neighbors down and up the column, wrapping into the next column.
        let stops = self.tab_stops(stars);
        let total = rows * stops.len();
        let at = |pos: usize| {
            let (col, row) = (pos / rows, pos % rows);
            self.cell(stops[col], self.order[row])
                .unwrap_or_else(|| cell.clone())
                .read(cx)
                .focus_handle(cx)
        };
        let step = |from: usize, dir: i64| (from as i64 + dir).rem_euclid(total as i64) as usize;
        let pos = stops.iter().position(|stop| *stop == kind).unwrap_or(0) * rows + row_ix;
        let next = at(step(pos, 1));
        let prev = at(step(pos, -1));
        // Tab moves down the column: this binding catches it deeper than the
        // root's.
        div()
            .key_context("TagField")
            .on_action({
                let cell = cell.clone();
                move |_: &FieldTab, window, cx| accept_then_focus(&cell, &next, window, cx)
            })
            .on_action({
                let cell = cell.clone();
                move |_: &FieldTabPrev, window, cx| accept_then_focus(&cell, &prev, window, cx)
            })
            .child(Input::new(&cell).small().appearance(false))
            .into_any_element()
    }
}

/// Keys ordered by how many files have them, alphabetical in a tie, so the
/// order holds across opens. The table's tag columns run in this order.
fn build_additional(
    reads: &[FileRead],
    window: &mut Window,
    cx: &mut Context<TagEditor>,
) -> AdditionalTags {
    let mut gathered: Vec<(String, Vec<UnknownValue>, Vec<usize>)> = Vec::new();
    let mut failed = 0;
    for (ix, read) in reads.iter().enumerate() {
        let FileRead::Read { unknown, .. } = read else {
            continue;
        };
        let Ok(rows) = unknown else {
            failed += 1;
            continue;
        };
        for (key, value) in rows {
            match gathered.iter_mut().find(|(k, _, _)| k == key) {
                Some((_, values, files)) => {
                    values.push(value.clone());
                    if files.last() != Some(&ix) {
                        files.push(ix);
                    }
                }
                None => gathered.push((key.clone(), vec![value.clone()], vec![ix])),
            }
        }
    }
    gathered.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then_with(|| a.0.cmp(&b.0)));
    let rows = gathered
        .into_iter()
        .map(|(key, values, files)| {
            let agreed = values.windows(2).all(|pair| pair[0] == pair[1]);
            let label: SharedString = one_line(&key).into();
            let files = files.len();
            if values.iter().any(|v| matches!(v, UnknownValue::Binary(_))) {
                let size = if agreed {
                    one_line(&values[0].display())
                } else {
                    "Multiple values".to_owned()
                };
                return AdditionalRow {
                    key,
                    label,
                    key_input: None,
                    initial: SharedString::default(),
                    input: None,
                    binary: Some(size.into()),
                    files,
                    removed: false,
                    mixed: false,
                };
            }
            let (value, placeholder) = if agreed {
                (values[0].display(), "")
            } else {
                (String::new(), "Multiple values")
            };
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            });
            let initial = input.read(cx).value().clone();
            AdditionalRow {
                key,
                label,
                key_input: None,
                initial,
                input: Some(input),
                binary: None,
                files,
                removed: false,
                mixed: !agreed,
            }
        })
        .collect::<Vec<_>>();
    AdditionalTags {
        // Authored rows land after these, past the count.
        columns: rows.len(),
        rows,
        failed,
        files: reads.len(),
    }
}

/// A lyric sheet or a json blob is a tag too and still has to fit a row.
fn one_line(value: &str) -> String {
    const LIMIT: usize = 240;
    let flat: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LIMIT {
        return flat;
    }
    let cut: String = flat.chars().take(LIMIT).collect();
    format!("{cut}...")
}

fn leading_number(value: &str) -> u32 {
    let digits: String = value
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().unwrap_or(0)
}

impl Render for TagEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: gpui::AnyElement = if self.table {
            div()
                .size_full()
                .flex()
                .flex_col()
                .p(tokens::SPACE_MD)
                .child(self.tags_section(cx).flex_1().min_h_0())
                .children(
                    self.additional_section(cx)
                        .map(|section| section.flex_none().mt(SECTION_GAP)),
                )
                .into_any_element()
        } else {
            div()
                .id("tag-editor-page")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .p(tokens::SPACE_MD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SECTION_GAP)
                        .child(self.tags_section(cx))
                        .children(self.additional_section(cx)),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div().flex_1().min_h_0().flex().flex_row().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        .bg(palette::bg_elevated())
                        .child(page)
                        .when(!self.table, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Scrollbar::vertical(&self.scroll)),
                            )
                        }),
                ),
            )
            .child(self.footer(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ColumnKind, FIELDS, LABEL_MIN_W, LEAD, OPT_IN_COLUMNS, TagColumn, TagIntent,
        baseline_value, change_for, column_keys, column_kind, column_rank, column_shown,
        default_widths, file_tag_key, fills_sort_field, fold_cell, fold_tag_intents, form_fields,
        grid_columns, label_column_w, placed_widths, shared_value, sort_columns_to_show,
        sort_field, tag_change_for, tag_key_of, title_case,
    };
    use rox_library::writer::{Field, UnknownValue};
    use std::collections::{BTreeMap, HashSet};

    fn baseline(pairs: &[(Field, &str)]) -> Vec<(Field, String)> {
        pairs
            .iter()
            .map(|(field, value)| (field.clone(), (*value).to_string()))
            .collect()
    }

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, UnknownValue)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), UnknownValue::Text((*value).to_string())))
            .collect()
    }

    fn tag_columns(keys: &[&str]) -> Vec<TagColumn> {
        keys.iter()
            .map(|key| TagColumn {
                key: (*key).to_string(),
                name: (*key).to_string().into(),
                text: true,
            })
            .collect()
    }

    fn set(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    #[test]
    fn sort_fields_follow_their_base_field() {
        let pairs: Vec<(&Field, &Field)> = FIELDS
            .windows(2)
            .filter(|w| {
                matches!(
                    w[1].0,
                    Field::TitleSort
                        | Field::ArtistSort
                        | Field::AlbumArtistSort
                        | Field::AlbumSort
                )
            })
            .map(|w| (&w[0].0, &w[1].0))
            .collect();
        assert!(pairs.len() == 4);
        for (base, sort) in pairs {
            let expected = match sort {
                Field::TitleSort => Field::Title,
                Field::ArtistSort => Field::Artist,
                Field::AlbumArtistSort => Field::AlbumArtist,
                _ => Field::Album,
            };
            assert!(base == &expected);
        }
        let per_track = |field: &Field| FIELDS.iter().find(|(f, _, _)| f == field).unwrap().2;
        assert!(per_track(&Field::TitleSort));
        assert!(!per_track(&Field::ArtistSort));
        assert!(!per_track(&Field::AlbumArtistSort));
        assert!(!per_track(&Field::AlbumSort));
    }

    /// One file carries an artist sort name and one doesn't: mixed, and arming
    /// it writes both.
    #[test]
    fn a_half_tagged_batch_reads_mixed_and_arms_both_files() {
        let tagged = baseline(&[
            (Field::Artist, "米津玄師"),
            (Field::ArtistSort, "Yonezu, Kenshi"),
        ]);
        let untagged = baseline(&[(Field::Artist, "米津玄師")]);
        let baselines = vec![tagged.clone(), untagged.clone()];

        let (value, mixed) = shared_value(&Field::ArtistSort, &baselines);
        assert!(mixed);
        assert!(value.is_empty());
        let (value, mixed) = shared_value(&Field::Artist, &baselines);
        assert!(!mixed);
        assert!(value == "米津玄師");

        let armed = "Yonezu Kenshi".to_string();
        for base in [&tagged, &untagged] {
            let change = change_for(&Field::ArtistSort, armed.clone(), base)
                .expect("an armed field writes to every file it doesn't already match");
            assert!(change.field == Field::ArtistSort);
            assert!(change.value.as_deref() == Some("Yonezu Kenshi"));
        }
    }

    #[test]
    fn an_untouched_sort_field_writes_nothing() {
        let tagged = baseline(&[(Field::AlbumSort, "Lemon")]);
        let untagged = baseline(&[(Field::Album, "レモン")]);

        assert!(change_for(&Field::AlbumSort, "Lemon".into(), &tagged).is_none());
        assert!(change_for(&Field::AlbumSort, String::new(), &untagged).is_none());
        assert!(baseline_value(&untagged, &Field::AlbumSort).is_empty());

        let cleared = change_for(&Field::AlbumSort, String::new(), &tagged)
            .expect("emptying a carried tag drops it");
        assert!(cleared.value.is_none());
    }

    #[test]
    fn a_column_resolves_to_one_kind() {
        let tags = tag_columns(&["MOOD", "ISRC"]);
        assert!(column_kind("file", &tags) == Some(ColumnKind::File));
        assert!(column_kind("album artist", &tags) == Some(ColumnKind::Field(4)));
        assert!(column_kind("tag:MOOD", &tags) == Some(ColumnKind::Tag(0)));
        assert!(column_kind("tag:ISRC", &tags) == Some(ColumnKind::Tag(1)));
        assert!(column_kind("tag:GONE", &tags).is_none());
        assert!(column_kind("nonsense", &tags).is_none());
        let shadow = tag_columns(&["album"]);
        assert!(column_kind("tag:album", &shadow) == Some(ColumnKind::Tag(0)));
        assert!(column_kind("album", &shadow) == Some(ColumnKind::Field(6)));
    }

    #[test]
    fn tag_columns_rank_after_the_fields() {
        let tags = tag_columns(&["MOOD", "ISRC"]);
        assert!(column_rank("file", &tags) == Some(0));
        assert!(column_rank("title", &tags) == Some(LEAD));
        assert!(column_rank("tag:MOOD", &tags) == Some(LEAD + FIELDS.len()));
        assert!(column_rank("tag:ISRC", &tags) == Some(LEAD + FIELDS.len() + 1));
    }

    #[test]
    fn sort_and_tag_columns_start_off() {
        let (hidden, shown) = (set(&[]), set(&[]));
        for (_, label, _) in FIELDS {
            let asked = OPT_IN_COLUMNS.contains(label);
            assert!(column_shown(label, &hidden, &shown) != asked, "{label}");
        }
        assert!(column_shown("file", &hidden, &shown));
        assert!(!column_shown("tag:MOOD", &hidden, &shown));

        let shown = set(&["album sort", "tag:MOOD"]);
        let hidden = set(&["genre"]);
        assert!(column_shown("album sort", &hidden, &shown));
        assert!(column_shown("tag:MOOD", &hidden, &shown));
        assert!(!column_shown("title sort", &hidden, &shown));
        assert!(!column_shown("genre", &hidden, &shown));
    }

    #[test]
    fn widths_from_before_the_sort_columns_are_placed() {
        let defaults = default_widths();
        let old: Vec<f32> = column_keys()
            .filter(|key| !sort_field(key))
            .enumerate()
            .map(|(i, _)| 100. + i as f32)
            .collect();
        assert!(old.len() + OPT_IN_COLUMNS.len() == defaults.len());

        let placed = placed_widths(&old).expect("the old layout is one this build knows");
        assert!(placed.len() == defaults.len());
        let mut taken = old.iter();
        for (i, key) in column_keys().enumerate() {
            if sort_field(key) {
                assert!(placed[i] == defaults[i], "{key}");
            } else {
                assert!(
                    placed[i] == *taken.next().expect("one old slot per kept column"),
                    "{key}"
                );
            }
        }
        assert!(placed_widths(&defaults).as_deref() == Some(defaults.as_slice()));
        assert!(placed_widths(&[]).is_none());
        assert!(placed_widths(&old[..old.len() - 1]).is_none());

        let shown = set(&["title sort"]);
        let hidden = set(&[]);
        let columns = grid_columns(&old, &BTreeMap::new(), &[], &hidden, &shown);
        let width = |key: &str| -> f32 {
            columns
                .iter()
                .find(|column| column.key.as_ref() == key)
                .map(|column| column.width.into())
                .expect("the column is shown")
        };
        assert!(width("file") == old[0]);
        assert!(width("title") == old[1]);
        assert!(width("title sort") == defaults[2]);
        assert!(width("artist") == old[2]);
    }

    #[test]
    fn tag_widths_are_kept_by_key_not_by_slot() {
        let saved = default_widths();
        let mut widths = BTreeMap::new();
        widths.insert("MOOD".to_string(), 300.);
        let shown = set(&["tag:MOOD", "tag:ISRC"]);
        let hidden = set(&[]);

        let both = grid_columns(
            &saved,
            &widths,
            &tag_columns(&["MOOD", "ISRC"]),
            &hidden,
            &shown,
        );
        let width = |columns: &[super::Column], key: &str| -> f32 {
            columns
                .iter()
                .find(|column| column.key.as_ref() == key)
                .map(|column| column.width.into())
                .expect("the column is shown")
        };
        assert!(width(&both, "tag:MOOD") == 300.);
        assert!(width(&both, "tag:ISRC") == super::TAG_WIDTH);
        let one = grid_columns(&saved, &widths, &tag_columns(&["ISRC"]), &hidden, &shown);
        assert!(one.iter().all(|column| column.key.as_ref() != "tag:MOOD"));
        assert!(width(&one, "tag:ISRC") == super::TAG_WIDTH);
        for columns in [&both, &one] {
            assert!(width(columns, "file") == saved[0]);
            assert!(width(columns, "title") == saved[LEAD]);
        }
    }

    #[test]
    fn keys_a_field_owns_are_refused() {
        for key in [
            "TITLE",
            "title",
            "TIT2",
            "TRACKNUMBER",
            "AlbumArtist",
            "TSOP",
        ] {
            assert!(tag_key_of(key).is_none(), "{key}");
        }
        assert!(tag_key_of("  MOOD  ") == Some("MOOD".to_string()));
        assert!(tag_key_of("REPLAYGAIN_TRACK_GAIN").is_some());
        assert!(tag_key_of("").is_none());
        assert!(tag_key_of("   ").is_none());
    }

    /// The refusals are the authored row's alone.
    #[test]
    fn a_file_spells_its_own_key() {
        for key in ["ALBUMARTISTSORT", "DATE", " MOOD "] {
            assert!(tag_key_of(key) != Some(key.to_string()), "{key}");
            assert!(file_tag_key(key) == Some(key.to_string()), "{key}");
        }
        assert!(file_tag_key("").is_none());

        let file = tags(&[(" MOOD ", "calm")]);
        let key = file_tag_key(" MOOD ").expect("a key off a file");
        let change = tag_change_for(&key, &TagIntent::Drop, &file).expect("the row removes");
        assert!(change.field == Field::Unknown(" MOOD ".to_string()));
        assert!(change.value.is_none());
        assert!(tag_change_for("MOOD", &TagIntent::Drop, &file).is_none());
    }

    #[test]
    fn an_authored_key_writes_once_per_file() {
        let files = [tags(&[("MOOD", "calm")]), tags(&[])];
        let key = tag_key_of("ISRC").expect("a key no field owns");
        let intent = TagIntent::Set("USRC17607839".to_string());
        for file in &files {
            let changes = fold_tag_intents(vec![(key.clone(), intent.clone())]);
            assert!(changes.len() == 1);
            let change = tag_change_for(&changes[0].0, &changes[0].1, file)
                .expect("a key the file doesn't spell that way is a change");
            assert!(change.field == Field::Unknown("ISRC".to_string()));
            assert!(change.value.as_deref() == Some("USRC17607839"));
        }
    }

    #[test]
    fn two_rows_on_one_key_fold_into_one() {
        let read = ("MOOD".to_string(), TagIntent::Keep);
        let authored = ("MOOD".to_string(), TagIntent::Set("restless".to_string()));
        let folded = fold_tag_intents(vec![read.clone(), authored.clone()]);
        assert!(folded == vec![("MOOD".to_string(), TagIntent::Set("restless".to_string()))]);

        let folded = fold_tag_intents(vec![authored, read]);
        assert!(folded.len() == 1);
        assert!(folded[0].1 == TagIntent::Set("restless".to_string()));

        let file = tags(&[("MOOD", "calm")]);
        let changes: Vec<_> = folded
            .iter()
            .filter_map(|(key, intent)| tag_change_for(key, intent, &file))
            .collect();
        assert!(changes.len() == 1);
    }

    #[test]
    fn an_untouched_tag_row_writes_nothing() {
        let carrier = tags(&[("MOOD", "calm")]);
        let bystander = tags(&[("ISRC", "USRC17607839")]);

        assert!(tag_change_for("MOOD", &TagIntent::Keep, &carrier).is_none());
        assert!(tag_change_for("MOOD", &TagIntent::Drop, &bystander).is_none());
        assert!(tag_change_for("MOOD", &TagIntent::Set("calm".into()), &carrier).is_none());
        assert!(tag_change_for("MOOD", &TagIntent::Set(String::new()), &bystander).is_none());

        let dropped = tag_change_for("MOOD", &TagIntent::Drop, &carrier)
            .expect("an armed removal drops the key from its carriers");
        assert!(dropped.value.is_none());
        let emptied = tag_change_for("MOOD", &TagIntent::Set(String::new()), &carrier)
            .expect("an emptied value drops the tag, like a field's");
        assert!(emptied.value.is_none());
    }

    #[test]
    fn a_drifted_form_folds_into_untouched_cells_only() {
        let folded = fold_cell("Lemon", "Lemon", "Lemon", Some("レモン"));
        assert!(folded == Some("レモン".into()));
        assert!(fold_cell("Kenshi", "Lemon", "Lemon", Some("レモン")).is_none());
        assert!(fold_cell("", "", "Lemon", None) == Some("Lemon".into()));
        // Why the column only re-seeds on the first build or under live drift.
        assert!(fold_cell("レモン", "レモン", "Lemon", None) == Some("Lemon".into()));
    }

    #[test]
    fn every_field_label_reads_as_a_label() {
        for (_, label, _) in FIELDS {
            let cased = title_case(label);
            assert!(!cased.contains("  "), "{cased}");
            assert!(
                cased.split(' ').count() == label.split(' ').count(),
                "{cased}"
            );
            for word in cased.split(' ') {
                let first = word.chars().next().expect("a label has no empty words");
                assert!(!first.is_lowercase(), "{cased}");
            }
            assert!(cased.to_lowercase() == *label, "{cased}");
        }
        assert!(title_case("album artist sort") == "Album Artist Sort");
    }

    #[test]
    fn the_sort_toggle_folds_away_four_rows() {
        let all = form_fields(true);
        assert!(all == (0..FIELDS.len()).collect::<Vec<_>>());

        let folded = form_fields(false);
        let dropped: Vec<&str> = (0..FIELDS.len())
            .filter(|i| !folded.contains(i))
            .map(|i| FIELDS[i].1)
            .collect();
        assert!(dropped == OPT_IN_COLUMNS, "{dropped:?}");
        assert!(folded.iter().all(|i| !sort_field(FIELDS[*i].1)));
    }

    #[test]
    fn a_filled_sort_name_opens_the_rows() {
        let named = [
            (Field::Artist, "米津玄師".to_string()),
            (Field::ArtistSort, "Yonezu, Kenshi".to_string()),
        ];
        assert!(fills_sort_field(&named));

        let plain = [(Field::Artist, "米津玄師".to_string())];
        assert!(!fills_sort_field(&plain));
        let blank = [(Field::AlbumArtistSort, "   ".to_string())];
        assert!(!fills_sort_field(&blank));
    }

    #[test]
    fn a_filled_sort_name_opens_its_column() {
        let filled = [
            (Field::Artist, "米津玄師".to_string()),
            (Field::ArtistSort, "Yonezu, Kenshi".to_string()),
            (Field::AlbumSort, String::new()),
        ];
        assert!(sort_columns_to_show(&filled, &set(&[])) == vec!["artist sort"]);
        assert!(sort_columns_to_show(&filled, &set(&["artist sort"])).is_empty());
        let both = [
            (Field::TitleSort, "Lemon".to_string()),
            (Field::AlbumArtistSort, "Yonezu, Kenshi".to_string()),
        ];
        assert!(sort_columns_to_show(&both, &set(&[])) == vec!["title sort", "album artist sort"]);
        let plain = [(Field::Album, "レモン".to_string())];
        assert!(sort_columns_to_show(&plain, &set(&[])).is_empty());
    }

    #[test]
    fn the_label_column_fits_its_widest_label() {
        let widest = |rows: &[usize]| {
            rows.iter()
                .map(|i| title_case(FIELDS[*i].1).chars().count())
                .max()
                .unwrap_or(0)
        };
        for rows in [form_fields(true), form_fields(false)] {
            let w = label_column_w(&rows);
            assert!(w >= LABEL_MIN_W);
            assert!(w >= widest(&rows) as f32 * super::LABEL_CHAR_W);
        }
        assert!(label_column_w(&form_fields(true)) > label_column_w(&form_fields(false)));
        assert!(label_column_w(&[]) == LABEL_MIN_W);
    }
}
