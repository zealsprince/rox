//! The tag editor window: one OS window opened on a selection (albums
//! picked in the grid, tracks picked in the library) rather than a panel,
//! since editing needs room and a plain close-without-saving story. One
//! shared field form covers the selection: a field every
//! file agrees on shows its value, differing values show empty over a
//! "multiple values" placeholder, and only the fields the user moves
//! write anything. Table mode swaps the form for one row of cells per
//! track, where the per-track fields a batch form has to lock stay
//! editable and tab steps through the grid. The name fields suggest the
//! library's own values as they're typed. Baselines come off each file
//! through the writer's read,
//! the metadata panel's convention, so every save diffs per file against
//! what that file actually has and commits through the atomic layer.
//! A successful save applies to the catalog in one batch, then re-reads the
//! written files so their rows converge with what's on disk, duration and
//! the rest the form never named included.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    actions, div, prelude::*, px, size, svg, App, Bounds, Context, Div, Entity, FocusHandle,
    Focusable as _, Global, KeyBinding, MouseButton, ScrollHandle, SharedString, Stateful,
    Subscription, WeakEntity, Window, WindowHandle,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Root, Sizable, Size};

use rox_library::cue::TrackKey;
use rox_library::rating;
use rox_library::writer::{self, Change, Edit, Field, UnknownValue};

use crate::matching::{open_or_focus, WindowRegistry};
use crate::tags::guess;
use rox_core::settings::{rating_style, RatingStyle, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers;
use rox_panel_api::panel::AppState;
use rox_panel_api::suggest;
use rox_panel_kit::ui::{
    self as settings_ui, kbd_line, section, section_with_control, Seg, SECTION_GAP,
};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// The form's fields in sheet order: the label each row shows, and
/// whether the field is per-track by nature. Per-track fields only edit
/// while a single track is selected; a batch would stamp one title or
/// track number over every file.
///
/// Each sort field sits under the field it sorts, and carries that
/// field's per-track bool: a sort title is per-track for the same reason
/// a title is, while an artist sort name is shared, so typing one
/// romanization fixes the whole selection at once.
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
    // Shared, since rating an album's files in one stroke is the batch
    // case the user asked for. The value is the writer's 0-10 number,
    // half points included.
    (Field::Rating, "rating", false),
];

/// The most files a save writes at once. A commit is a clone, a verify,
/// and a flush, so it's mostly disk rather than CPU, and past a handful of
/// them in flight the drive is the job; the convert and analysis pools cap
/// themselves at the same place for the same reason.
const SAVE_WORKERS: usize = 4;

/// How many display columns lead the table ahead of the editable
/// [`FIELDS`] grid in the full column order. The file column is one of
/// these: it has no input and no field, and a save never sees it.
/// The settings file's width slots are positional over this full order,
/// hidden columns included, so a width is kept when its column is
/// toggled away and back.
const LEAD: usize = 1;

/// The prefix a table column key carries when it addresses an
/// additional tag rather than a field. Tag keys are whatever the file
/// spells them, so without a prefix a stray tag spelled "album" would
/// answer to the album field's column key, and the writer would edit
/// the album behind the album row's back.
const TAG_PREFIX: &str = "tag:";

/// A tag column's width when nothing has sized it, the same as a name
/// field's: a tag value is text of unknown length, and the numerics'
/// narrow default would cut most of them off.
const TAG_WIDTH: f32 = 150.;

/// The fixed columns that only show when they're asked for. The four
/// sort names are the second half of four fields and most files carry
/// none of them, so opening every table with all fourteen puts the
/// columns people came for off the right edge. Everything else in
/// [`FIELDS`] shows unless it's been hidden.
const OPT_IN_COLUMNS: &[&str] = &[
    "title sort",
    "artist sort",
    "album artist sort",
    "album sort",
];

/// Whether a column has to be asked for rather than hidden away: the
/// sort names above, and every additional tag, since a selection
/// carrying fifteen stray keys would otherwise open with fifteen
/// surprise columns.
fn opt_in_column(key: &str) -> bool {
    key.starts_with(TAG_PREFIX) || OPT_IN_COLUMNS.contains(&key)
}

/// Whether a [`FIELDS`] label names one of the four sort names. The
/// sheet folds these away behind its own toggle, the same four the
/// table makes you ask for and for the same reason: most files carry
/// none of them, so a selection reads as four empty rows otherwise.
fn sort_field(label: &str) -> bool {
    OPT_IN_COLUMNS.contains(&label)
}

/// Which [`FIELDS`] rows the sheet draws: all of them, or all but the
/// sort names while the toggle is off. An index is the row's slot in
/// [`FIELDS`], which is where its input, its fill and its mixed flag
/// sit too.
fn form_fields(sort_fields: bool) -> Vec<usize> {
    FIELDS
        .iter()
        .enumerate()
        .filter(|(_, (_, label, _))| sort_fields || !sort_field(label))
        .map(|(i, _)| i)
        .collect()
}

/// A label's width in the sheet's text, per character. The labels are
/// English literals off [`FIELDS`] rather than translated copy, so a
/// character count is the whole measurement; the real advance only
/// exists inside a paint, and the health window's count column
/// estimates the same way for the same reason.
const LABEL_CHAR_W: f32 = 7.5;

/// The narrowest the label column draws, whatever the labels say.
const LABEL_MIN_W: f32 = 84.;

/// How wide the label column has to be for the widest label the sheet
/// draws, so "Album Artist Sort" holds one line instead of wrapping
/// under itself. The input column takes whatever is left.
fn label_column_w(rows: &[usize]) -> f32 {
    rows.iter()
        .map(|i| title_case(FIELDS[*i].1).chars().count() as f32 * LABEL_CHAR_W)
        .fold(LABEL_MIN_W, f32::max)
        .ceil()
}

/// Whether a looked-up match brings a sort name with it. A fill lands
/// in the inputs whether the sort rows are folded away or not, so
/// without this the value would sit in a row nobody can see.
fn fills_sort_field(values: &[(Field, String)]) -> bool {
    values.iter().any(|(field, value)| {
        !value.trim().is_empty()
            && FIELDS
                .iter()
                .any(|(f, label, _)| f == field && sort_field(label))
    })
}

/// The sort columns a fill has to turn on: the table's answer to the
/// form's toggle. A fill lands in the named track's cells whether the
/// column is on or not, so a value under a column nobody asked for
/// would sit off screen. Sort columns are opt-in, so the shown set
/// alone says which are already up.
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

/// Whether a column is on screen, read off the two sets the editor
/// keeps: an ordinary field shows unless it's hidden, an opt-in one
/// shows only while it's shown. The column builder, the header menu,
/// and the toggle all ask this rather than each reading the sets their
/// own way.
fn column_shown(key: &str, hidden: &HashSet<String>, shown: &HashSet<String>) -> bool {
    if opt_in_column(key) {
        shown.contains(key)
    } else {
        !hidden.contains(key)
    }
}

/// A column heading from a field label, each word capitalized: "album
/// artist" reads as "Album Artist" over the table while the form keeps
/// the lowercase label.
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

/// Every table column's key in the full order: the file column, then one
/// per [`FIELDS`] entry under its label.
fn column_keys() -> impl Iterator<Item = &'static str> {
    std::iter::once("file").chain(FIELDS.iter().map(|(_, label, _)| *label))
}

/// A key's slot in the full column order, the position its width is
/// stored at in the settings file whether the column shows or not.
fn canonical_ix(key: &str) -> Option<usize> {
    column_keys().position(|k| k == key)
}

/// Every column's default width in the full order: the file column wide
/// for a name, numerics narrow, the rating wide enough for five stars or
/// the numeric strip.
fn default_widths() -> Vec<f32> {
    std::iter::once(220.)
        .chain(FIELDS.iter().map(|(field, _, _)| match field {
            Field::Year | Field::TrackNo | Field::DiscNo => 64.,
            Field::Rating => 96.,
            _ => 150.,
        }))
        .collect()
}

/// The saved width slots read into this build's layout, or None for a set
/// that can't be placed in it.
///
/// Widths are positional over the full column order, so a set written when
/// that order was a different length can't be read straight. One older
/// shape is worth translating rather than throwing away: the layout from
/// before the four sort-name columns, which is what every settings file
/// written until now holds. Its slots line up with this order once the
/// four new columns are skipped, since they were added among columns that
/// kept their relative places, so the migration is a walk down both. The
/// new columns take their defaults, having never been sized.
///
/// Any other length is a build nobody here can name, and it falls back to
/// the defaults whole rather than sliding a dozen widths one column over.
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

/// One additional tag as a table column: the key it addresses, the
/// heading it draws, and whether its cells edit. A binary payload's
/// don't; the row shows a size and only removes, and a column of them
/// is the same read-only thing spread sideways.
#[derive(Clone)]
struct TagColumn {
    key: String,
    name: SharedString,
    text: bool,
}

/// What a table column edits: the file name, which it can't, a
/// [`FIELDS`] slot, or an additional tag by its place in the tag order.
/// Every column resolves to one of these once and the rest of the table
/// matches on the answer, so a tag column can't fall through to the
/// file column's branch the way the old Option<usize> let it (it would
/// have sorted the grid by file name).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColumnKind {
    File,
    Field(usize),
    Tag(usize),
}

/// A column key resolved against the field table and a tag order. None
/// for a key that names neither, which only a settings file edited by
/// hand can produce; its cells draw empty rather than guessing.
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

/// A column's place in the full order: the fixed columns in
/// [`column_keys`] order, then the additional tags in the order the
/// section lists them, so a column toggled back on lands where it left.
fn column_rank(key: &str, tags: &[TagColumn]) -> Option<usize> {
    if let Some(ix) = canonical_ix(key) {
        return Some(ix);
    }
    let tag = key.strip_prefix(TAG_PREFIX)?;
    let at = tags.iter().position(|column| column.key == tag)?;
    Some(LEAD + FIELDS.len() + at)
}

/// The field a tag key would edit behind that field's back, if any. The
/// writer maps a key like TITLE or TRACKNUMBER onto the same item the
/// title and track rows own, so a save through the additional list
/// would rewrite the field's own tag while the field's box sat there
/// saying something else. The editor refuses those keys rather than
/// letting two surfaces write one tag.
///
/// Matched on the key's letters alone, so TITLE, Title, and the ID3
/// frame id TIT2 all land on the same row. The alias list is the
/// spellings a person actually types; it isn't lofty's full mapping,
/// and it doesn't have to be, since the writer stays correct either
/// way and this only decides what the editor talks the user out of.
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

/// The key an additional row writes under, or None for a row a save has
/// no business acting on: a blank key addresses nothing and the writer
/// would take it seriously, and a key a field already owns is refused
/// here rather than written twice.
fn tag_key_of(raw: &str) -> Option<String> {
    let key = raw.trim();
    (!key.is_empty() && field_owning(key).is_none()).then(|| key.to_owned())
}

/// The key a row read off a file writes under: the one the file spells,
/// byte for byte.
///
/// Neither refusal above applies here. A file is free to carry a TXXX
/// called ALBUMARTISTSORT or one whose description has a space on the end,
/// and the row for it is the only place that tag can be edited or removed;
/// trimming the key or refusing it for folding to a field's label would
/// draw the row and then silently skip it at save, because the baseline and
/// the writer's verify both address it by the exact string.
fn file_tag_key(key: &str) -> Option<String> {
    (!key.is_empty()).then(|| key.to_owned())
}

/// What one additional row asks of one file: nothing when the row was
/// left alone, the key gone when its removal is armed, or a value in
/// hand, from the shared input or from that file's own cell.
#[derive(Clone, PartialEq, Eq, Debug)]
enum TagIntent {
    Keep,
    Drop,
    Set(String),
}

/// Two rows on one key fold into one change. The later row wins, since
/// it's the one just authored and the writer applies changes in order,
/// so a second change on the key would silently win anyway; writing
/// both would only make which one landed harder to read. A row asking
/// for nothing never erases one that asks for something.
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

/// The change one additional row contributes for one file, diffed
/// against that file's own read: None when the file already spells the
/// key that way, when the row asks for nothing, or when a removal names
/// a key the file never carried. An emptied value drops the tag, the
/// same as a field's.
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

/// ADR 18's last-edit-wins rule for one cell: what it should hold once
/// the form's value in flight is folded in. `form` is the form's value
/// when it drifted from what filled it and None when it didn't, `seed`
/// is what this cell last took from a fold, and `base` is the file's
/// own baseline.
///
/// None means leave the cell alone, which is what a cell the user
/// already moved gets: their value is the newest typing for that file
/// and the form's is older. Some is the value to hold and the seed to
/// record. Fields and additional tags both read the rule here rather
/// than each carrying their own version of it.
fn fold_cell(current: &str, seed: &str, base: &str, form: Option<&str>) -> Option<SharedString> {
    if current != seed {
        return None;
    }
    Some(SharedString::from(form.unwrap_or(base).to_owned()))
}

/// What one file's baseline says a field holds, empty when the writer's
/// read found no such tag on it. The whole diff hangs off this: a field
/// the file never carried and a field the user emptied both read as "",
/// which is what makes an untouched empty row cost nothing.
fn baseline_value<'a>(baseline: &'a [(Field, String)], field: &Field) -> &'a str {
    baseline
        .iter()
        .find(|(f, _)| f == field)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// What a field fills with over a batch, and whether the files disagree.
/// A field every file spells the same shows that value; a split one
/// fills empty so the mixed placeholder can say so. Multi-value tags
/// count their first item, the same one the writer's verify reads back.
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

/// The change one field contributes for one file: None when the value in
/// hand already matches that file's own baseline, so an unchanged field
/// never rewrites. An emptied value drops the tag.
fn change_for(field: &Field, value: String, baseline: &[(Field, String)]) -> Option<Change> {
    if value == baseline_value(baseline, field) {
        return None;
    }
    Some(Change {
        field: field.clone(),
        value: (!value.is_empty()).then_some(value),
    })
}

/// What one file's read says an additional key holds: the text under
/// it, and empty for a key the file doesn't carry, for a binary payload
/// (which never edits), and for a file whose read failed, whose
/// additional tags a save leaves alone anyway.
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

/// [`fold_cell`] over one column of cells, with the entity plumbing:
/// every cell still on its seed takes the form's drifted value, or its
/// own file's baseline when the form is quiet, while a cell the user
/// moved keeps what they typed. `reseed` is the grid's first build,
/// where there's nothing to protect yet.
///
/// Returns whether the form value was the drifted one, so the caller
/// can stop counting it as form drift once the cells hold it.
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
    // Once the grid exists the cells hold the truth: a re-entry only
    // folds in live form drift. Re-seeding a quiet column would push
    // its cells back to the disk baseline, wiping the values an earlier
    // fold-in brought in.
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

/// A path as the row shows it: the file name alone, the whole path when
/// there's no name to take.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The rating inputs' empty-state hint, the one field whose scale isn't
/// obvious from its label.
fn field_placeholder(field: &Field) -> &'static str {
    match field {
        Field::Rating => "0-10",
        _ => "",
    }
}

/// The rating field's face over its editor input: the shared rating
/// control. A click writes the display number into the input, so the
/// diff, mixed, and save paths see it like any typed field.
fn rating_field(input: &Entity<InputState>, cx: &App) -> Div {
    let current = rating::parse_display(input.read(cx).value().trim()).unwrap_or(0);
    let input = input.clone();
    // The input's entity id keys the hover preview; unlike a track id
    // it's unique per editor row.
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

actions!(tag_editor, [FieldTab, FieldTabPrev, Save]);

/// The key context the window root's own bindings scope to.
const CONTEXT: &str = "TagEditor";

/// The editor's bindings; call once at startup. The tab pair scopes to
/// the field wrappers' key context, deeper along the focus path than the
/// window root's own tab bindings, so inside a tag field the editor owns
/// what tab means: take the open suggestion, then move. Bindings win
/// over key listeners, so a listener could never have seen the key.
///
/// Enter is bound on the window root instead, so it saves from a field, a
/// table cell, or nothing focused at all. The inputs see the key first,
/// their own binding being deeper: a single-line input propagates it up
/// to here, and an open suggestion menu swallows it, so enter takes the
/// suggestion first and saves on the next press.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FieldTab, Some("TagField")),
        KeyBinding::new("shift-tab", FieldTabPrev, Some("TagField")),
        KeyBinding::new("enter", Save, Some(CONTEXT)),
    ]);
}

/// Take the open suggestion off `input` without firing its own enter.
/// Routing the enter straight to the completion menu accepts a suggestion
/// when one is up and does nothing when it isn't. Dispatching the input's
/// Enter action instead would, with no menu open, emit PressEnter, which
/// the save subscription reads as a save and closes the window. That's
/// the tab-closes-the-window bug.
fn take_suggestion(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    input.update(cx, |state, cx| {
        state.handle_action_for_context_menu(Box::new(Enter { secondary: false }), window, cx);
    });
}

/// Take the open suggestion, then move focus to `target`.
fn accept_then_focus(
    input: &Entity<InputState>,
    target: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    take_suggestion(input, window, cx);
    window.focus(target);
    // Accepting a suggestion calls propagate on the menu, which would let
    // the keystroke reach the window root's own tab binding for a second
    // focus move. Stop it explicitly.
    cx.stop_propagation();
}

/// The open editors, each keyed by the sorted ids it opened on: every
/// selection edits in its own window, and asking for one already open
/// focuses that window instead of stacking a twin, since an edit in
/// progress isn't worth losing.
#[derive(Default)]
struct OpenTagEditors(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenTagEditors {}

impl WindowRegistry for OpenTagEditors {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a tag editor on `ids`, the selection's tracks in view order, or
/// bring the editor already on that selection to the front. An empty
/// selection opens nothing.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    open_or_focus::<OpenTagEditors>(
        key,
        move |cx| {
            // The last closed editor's size, sanity-floored; the default is
            // wide enough that the table's columns fit without scrolling.
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

/// One selected track, resolved at open; the baselines read the path and
/// the commits write it, and the sub says which row of it they
/// belong to when the file is a cue image. The title only names the
/// track in errors. The table's file column is where the selection
/// shows itself.
struct TrackRow {
    path: PathBuf,
    sub: u16,
    title: SharedString,
}

/// One file's reads off the background hop: the fields the form edits
/// and the tags it only shows, or the note that the writer has no path
/// for this format at all.
enum FileRead {
    Unsupported,
    Read {
        fields: Result<Vec<(Field, String)>, String>,
        unknown: Result<Vec<(String, UnknownValue)>, String>,
    },
}

/// The selection's tags that no field addresses, plus the ones the user
/// added here, unioned into one editable list. "Additional" rather than
/// "unknown" because after the add button the list holds rows nobody
/// failed to recognize; the writer's [`Field::Unknown`] keeps its own
/// name, where "a tag outside the editable set" is still exactly right.
struct AdditionalTags {
    rows: Vec<AdditionalRow>,
    /// How many of the leading rows came off the files. Those are the
    /// ones with a table column and per-file cells; the rows the user
    /// authored follow them and are batch-only until they're on disk,
    /// since a key still being typed has no per-file identity to hang a
    /// column on.
    columns: usize,
    /// How many files' tag reads failed. The list is short by that
    /// many, so the section says so rather than passing for complete,
    /// and save leaves those files' additional tags alone, since
    /// there's nothing safe to diff them against.
    failed: usize,
    /// How many files the union covers, for the per-row "3 of 7".
    files: usize,
}

/// One key in that list: the exact key a save addresses it by, the input
/// its text edits through, and how many of the selection have it.
struct AdditionalRow {
    /// The key as the file spells it, what [`Field::Unknown`] writes by.
    /// Empty on an authored row until its key input says otherwise.
    key: String,
    /// The key flattened to one row for the label.
    label: SharedString,
    /// The key's own editor on an authored row, None on a row read off
    /// a file, whose key is fixed by what the file spells. It sits in
    /// the same slot the fixed label occupies, so the two row kinds
    /// line up.
    key_input: Option<Entity<InputState>>,
    /// What the value input filled with: the value every carrier agrees
    /// on, empty under the mixed placeholder. An edit arms by drifting.
    initial: SharedString,
    /// The value's editor; a binary payload has none and only removes.
    input: Option<Entity<InputState>>,
    /// A binary payload's size line, shown in the input's place.
    binary: Option<SharedString>,
    files: usize,
    /// Armed to remove the key from every carrier on save.
    removed: bool,
}

impl AdditionalRow {
    /// The key this row writes under as it stands, reading an authored
    /// row's input rather than its (empty) stored key.
    ///
    /// An authored key goes through [`tag_key_of`], which refuses a blank
    /// one and one a field already writes; a key read off a file goes
    /// through [`file_tag_key`], which keeps it exactly as the file spells
    /// it.
    fn key(&self, cx: &App) -> Option<String> {
        match &self.key_input {
            Some(input) => tag_key_of(&input.read(cx).value()),
            None => file_tag_key(&self.key),
        }
    }
}

/// One track's cell under an additional tag column.
#[derive(Clone)]
enum TagCell {
    /// A text tag edits per file, like a field's cell.
    Edit(Entity<InputState>),
    /// A binary payload shows the size this file carries and nothing
    /// for a file that doesn't carry the key. Read-only either way:
    /// bytes never edited in the form and a column doesn't change that.
    Fixed(SharedString),
}

pub struct TagEditor {
    library: Entity<Library>,
    tracks: Vec<TrackRow>,
    /// Each file's fields as the writer read them, parallel to `tracks`:
    /// what save diffs against, per file. None until every read comes in
    /// (or never, when a file defeats the parser), and save stays inert
    /// without it.
    baselines: Option<Vec<Vec<(Field, String)>>>,
    /// What the form filled each input with once the baselines arrived:
    /// the value every file shares, or empty under the mixed
    /// placeholder. A field arms by drifting from this.
    filled: Vec<SharedString>,
    /// Whether each field's files disagreed at the last fill; the
    /// read-only per-track rows say so instead of faking one value.
    mixed: Vec<bool>,
    /// Whether the user armed a batch field to clear across every file.
    /// A mixed field is empty over its placeholder, so an empty input
    /// alone can't mean "wipe this tag on all of them". This flag does,
    /// and save writes the field empty even when nothing was typed.
    cleared: Vec<bool>,
    /// One input per entry of [`FIELDS`].
    inputs: Vec<Entity<InputState>>,
    /// Table mode: the shared form swapped for one row of cells per
    /// track, where the per-track fields a batch form has to lock stay
    /// editable.
    table: bool,
    /// The cell grid, `tracks` rows by [`FIELDS`] columns, built on the
    /// first switch to table mode.
    cells: Option<Vec<Vec<Entity<InputState>>>>,
    /// The table over the cells, built with them: the component owns the
    /// column widths and sort state, the delegate shares the cell
    /// entities, so save reads the same inputs the table shows.
    grid: Option<Entity<TableState<CellGrid>>>,
    /// The field columns toggled off the table, remembered through the
    /// settings file like the widths. A hidden column's cells are kept,
    /// so nothing typed there is lost to a toggle.
    hidden: HashSet<String>,
    /// The opt-in columns toggled on: the four sort names and the
    /// additional tags, which show only when they're asked for. Tag
    /// keys are stored under their `tag:` column key, so a key from a
    /// selection this editor never opened on survives the round trip.
    shown: HashSet<String>,
    /// The additional tags' cells, `tracks` rows by
    /// [`AdditionalTags::columns`] columns, built with `cells`. A tag
    /// is per file here the way a field is, so fixing one file's stray
    /// key doesn't stamp the batch.
    tag_cells: Option<Vec<Vec<TagCell>>>,
    /// What each cell last seeded from, by column then track. A cell
    /// still on its seed follows re-seeds (a form edit folding in); one
    /// the user moved is theirs.
    seeds: Vec<Vec<SharedString>>,
    /// The same, for the additional tags' cells.
    tag_seeds: Vec<Vec<SharedString>>,
    /// Whether the sheet draws the four sort rows. Off by default:
    /// most files carry no sort names, so the rows are four empty
    /// boxes between the fields the user came for. The table asks for
    /// its own through the column menu instead.
    sort_fields: bool,
    /// The guess panel is open: a filename pattern with a live preview
    /// of the values it would pull from every track's path.
    guess: bool,
    /// The guess pattern's input, remembered across editors through the
    /// settings file, since one library tends to one naming scheme.
    pattern: Entity<InputState>,
    /// The tags no field addresses, editable under their own fold.
    /// None until the reads come in; a file whose tag read failed
    /// only costs its own rows, never the form.
    additional: Option<AdditionalTags>,
    /// Each file's additional tags as the writer read them, parallel to
    /// `tracks`: what an additional edit diffs against per file. None
    /// where the read failed, and save leaves that file's tags alone.
    additional_baselines: Vec<Option<Vec<(String, UnknownValue)>>>,
    /// Whether that fold is open. Closed at open: most files have a few
    /// of these and some have a screenful.
    additional_open: bool,
    /// How many of the selection are in a format the writer has no path
    /// for. Those files say so plainly instead of showing a parse error
    /// over a dead form.
    unsupported: usize,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// A commit is in flight; the fields lock and the buttons hold still
    /// until it finishes.
    saving: bool,
    /// The save already ran and the window is on its way out. One enter
    /// press can reach [`Self::save`] twice (the focused input's own
    /// binding and the window root's, which the input propagates to), and
    /// a batch with nothing to write closes on the first without ever
    /// raising `saving` for the second to see.
    saved: bool,
    /// How many of the batch have committed and how many there are, for
    /// the "Saving n/m" count. A file at a time advances this, so a slow
    /// or stuck one shows where the batch is instead of a mute spinner.
    save_done: usize,
    save_total: usize,
    /// The page's scroll position, shared with the scrollbar.
    scroll: ScrollHandle,
    /// The shared art bake and this window's slice of the backdrop, so
    /// the window backs with the playing track's art like every other.
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl TagEditor {
    fn new(state: AppState, ids: Vec<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The list rows come off the projection where the library knows
        // the track; a file the projection misses still edits, its name
        // standing in for the title.
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
                        // The scale isn't free text; typing anything it
                        // can't parse never reaches the field.
                        input = input.validate(|s, _| {
                            s.trim().is_empty() || rating::parse_display(s).is_some()
                        });
                    }
                    input.lsp.completion_provider = suggest::provider(&state.library, field, cx);
                    input
                })
            })
            .collect();
        // Enter in any input saves, the metadata panel's convention. The
        // change repaint keeps the rating control on the typed value.
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
        // The guess pattern, seeded from the last editor's; enter applies
        // the guesses rather than saving, since the preview is right
        // there and an accidental save would close the window.
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
        // The OS close button never runs remove_window, so the frame
        // persists through the should-close hook; the save and cancel
        // paths call persist_frame themselves.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist_frame(window, cx));
            }
            true
        });
        // A multi-selection opens straight into the table, since that's
        // the per-track view; a single track fits the form.
        let table = tracks.len() > 1;
        // The columns the last editor toggled away and the ones it
        // toggled on, both pruned of anything that stopped being a
        // column since they were written. An opt-in column never sits
        // in the hidden set, and only opt-in columns sit in the shown
        // one, so a key that drifted between the two lists is dropped
        // rather than half honoured.
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

    /// Read every file's fields off the UI thread and fill the form when
    /// they all come in. One unreadable file blocks the whole save:
    /// without its baseline there's nothing safe to diff that file
    /// against. The read-only tags are read on the same hop, one file at
    /// a time, so the list costs nothing extra in wall time and a file
    /// that defeats it costs only its own rows.
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
                // Nothing here parses, so there's no form to fill and no
                // list to show; the section says which of the two it is.
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

    /// Open or close the additional tag list.
    fn toggle_additional(&mut self, cx: &mut Context<Self>) {
        self.additional_open = !self.additional_open;
        cx.notify();
    }

    /// Arm or disarm one additional key's removal: armed, save drops the
    /// key from every file that has it; disarmed, the row goes back
    /// to editing. Nothing touches disk until save, like everything else
    /// here.
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

    /// Append a blank row with its key open for typing, and open the
    /// fold so it's on screen. An authored row is batch-only: its value
    /// stamps every file in the selection, since a key still being
    /// typed has no per-file identity for the table to hang a column
    /// on. It joins the columns on the next open, once it's on disk and
    /// the read finds it like any other tag.
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
        // The key decides whether the row saves at all and says so
        // inline, so a keystroke in it has to repaint the section.
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
            });
        }
        self.additional_open = true;
        window.focus(&focus);
        cx.notify();
    }

    /// Fill the form off the landed baselines: a field every file agrees
    /// on shows its value, a differing one shows empty over the mixed
    /// placeholder. Multi-value tags count their first item, the same one
    /// the writer's verify reads back.
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
        // A table-first open can only build its cells once the baselines
        // arrive, so they seed here.
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

    /// Write the window frame and column widths into the settings file,
    /// the restore for the next editor. Runs on every close path; with
    /// several editors open the last writer wins.
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
        Settings::update(move |s| {
            let state = s.windows.tag_editor.get_or_insert_with(Default::default);
            state.width = frame.size.width.into();
            state.height = frame.size.height.into();
            // A form-only session has no table; keep the saved widths.
            // The shown columns write into their slots in the full order,
            // so a hidden column's width stays untouched. A tag column
            // writes under its key instead: the next selection carries a
            // different set of tags, and a slot would land its width on
            // whichever tag happened to sort into that place.
            if !columns.is_empty() {
                // Read through the same migration the table opened with,
                // so a hidden column's width survives the write instead of
                // being flattened to a default on the first resize.
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
        });
    }

    /// The first field the toggles leave on screen, where table focus
    /// goes: the title unless its column is off.
    fn first_visible_field(&self) -> usize {
        FIELDS
            .iter()
            .position(|(_, label, _)| column_shown(label, &self.hidden, &self.shown))
            .unwrap_or(0)
    }

    /// Show or hide a table column, keeping the rest in place. A shown
    /// column returns to its slot in the full order at its default
    /// width; hiding drops it, and never the last one, since an empty
    /// table has no header to bring one back from. Which set the toggle
    /// writes depends on the column: a field is hidden away, an
    /// additional tag or a sort name is asked for.
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
                // The table never reorders columns, so the shown set
                // stays in the full order and the rank places it.
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
                // A hidden sort column leaves no header to clear the
                // sort; drop back to the file order instead.
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

    /// Flip between the shared form and the per-track table. The table
    /// waits for the baselines the same way save does.
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

    /// Enter table mode: build the cell grid on first use, then seed
    /// every untouched cell with its file's baseline under any form edit
    /// in flight. A folded-in form edit stops counting as form drift
    /// (the cells hold it from here), and a cell the user already moved
    /// keeps their value.
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
                    // No save-on-enter here, unlike the form: enter in a
                    // cell accepts an open suggestion and nothing else,
                    // so tabbing through the grid can't fire a commit.
                    let input = cx.new(|cx| {
                        let mut input =
                            InputState::new(window, cx).placeholder(field_placeholder(field));
                        input.lsp.completion_provider = suggest::provider(&library, field, cx);
                        input
                    });
                    // A rating click writes to the cell's input; without
                    // this repaint the control would show the old value.
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
            // The additional tags become columns in the order the
            // section lists them, most files first then the key. The
            // rows the user authored stay out: their keys are still
            // being typed, so there's nothing per file to hang a column
            // on. They join on the next open, once they're on disk and
            // the read finds them like any other tag.
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
                        // A binary payload's size, this file's own: a
                        // column of them says which files carry the
                        // frame, which the union row can't.
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
            // The file column's names are shown in bare disabled inputs,
            // the track list's trick for text that has to select and copy.
            // Built with the grid, so a form-only session pays nothing.
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
            // The component owns the live column widths; copy a resize
            // into the delegate so a re-prepare keeps it, and the close
            // path persists it.
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
            // The cells hold the truth from here, so a pending clear-all
            // from the form is off. Left armed, save would wipe the tag
            // on every file while the table showed the original values.
            self.cleared[i] = false;
        }
        // The additional tags fold the same way, per ADR 18: one rule
        // for both, not a second one written for tags.
        for ix in 0..self.tag_seeds.len() {
            let Some((key, filled, input)) = self
                .additional
                .as_ref()
                .and_then(|a| a.rows.get(ix))
                .map(|row| (row.key.clone(), row.initial.clone(), row.input.clone()))
            else {
                continue;
            };
            // A binary row has no value input and no editable cells.
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
            if drifted {
                if let Some(row) = self.additional.as_mut().and_then(|a| a.rows.get_mut(ix)) {
                    row.initial = form_value.into();
                }
            }
        }
    }

    /// Leave table mode: the form re-reads the cells (a field the rows
    /// agree on shows the value, a split one goes back to empty over the
    /// mixed placeholder), and the fill snapshot follows, so only typing
    /// from here on counts as a bulk edit.
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
            // The table re-read is a fresh baseline, so any pending
            // clear-all from the form is off.
            self.cleared[i] = false;
        }
        // The additional tags come back the same way, so a tag the
        // table split between two files reads as mixed in the form
        // rather than as whichever file happened to be first.
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
            }
        }
    }

    /// Toggle a batch field's clear-all arm: on, the field wipes its tag
    /// across every file in the selection on save; off, it goes back to
    /// leaving the split values alone. Only the shared form's mixed fields
    /// get this; a single track just empties its box.
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

    /// Show or fold away the sheet's four sort rows. Remembered
    /// through the settings file, since a library either carries
    /// romanizations or it doesn't.
    fn toggle_sort_fields(&mut self, cx: &mut Context<Self>) {
        self.sort_fields = !self.sort_fields;
        cx.notify();
    }

    /// Show or hide the guess panel; opening moves focus to the pattern.
    fn toggle_guess(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.guess = !self.guess;
        if self.guess {
            window.focus(&self.pattern.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    /// Write the pattern's matches into the editor: per-track values go
    /// into the table's cells (switching to table mode to show them), a
    /// single track still on the form fills its fields. Either way the
    /// values arm like typing and nothing touches disk until save.
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
            // The seeds stay put: a guessed value reads as the user's own
            // edit, so re-entering the table never reseeds it away.
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

    /// The guess panel: the pattern input over a live preview of the
    /// values it pulls from each track's path, so the query's shape is
    /// visible before anything applies. Rows past the cap fold into a
    /// count; the apply button says how many tracks matched.
    fn guess_panel(&self, cx: &mut Context<Self>) -> Div {
        /// How many preview rows show before the rest fold into a count.
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
                                // Title cased like the sheet's rows and
                                // the table's headings, so one window
                                // doesn't name the same field two ways.
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
                            // Enter here applies the guesses, which the
                            // pattern's own subscription does; it stops
                            // short of the window root's save, since the
                            // preview is right there and a save would
                            // close the window out from under it.
                            .on_action(|_: &Save, _, cx: &mut App| cx.stop_propagation())
                            .child(Input::new(&self.pattern).small()),
                    )
                    .child(settings_ui::small_button(
                        "Apply",
                        icons::ARROW_DOWN,
                        self.saving || self.baselines.is_none() || hits == 0,
                        cx.listener(|this, _, window, cx| this.apply_guesses(window, cx)),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "tags-editor-guess-help",
                        placeholders = guess::PLACEHOLDERS.join(" ")
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

    /// Open the metadata compare on one edited track. The window
    /// searches, ranks matches, and on apply calls back into
    /// [`Self::fill_fields`] rather than writing, so this editor stays the
    /// one writer. A lookup is one track's by nature: the form's header
    /// button covers a single track, the table's rows one each.
    /// The compare keys its window on the track, so a row at a time can
    /// be open without the two fills crossing.
    fn look_up(&mut self, track: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.tracks.get(track) else {
            return;
        };
        let key = TrackKey {
            path: row.path.clone(),
            sub: row.sub,
        };
        let library = self.library.clone();
        let now_art = self.now_art.clone();
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        crate::tags::matcher::open_fill(library, now_art, key, track, weak, handle, cx);
    }

    /// Fill from a looked-up match, one field at a time: each set input
    /// drifts from its fill and arms as a pending edit, so the normal
    /// save writes it and nothing reaches disk until the user saves.
    /// Fields the match doesn't have are left untouched. The compare
    /// calls this on its own apply, on this editor's window, naming the
    /// track it ran on.
    ///
    /// The values go where the user can see them: the named
    /// track's cells once the grid is up, the shared form only in a
    /// form-only single-track session, since a batch form would stamp
    /// one track's release over every file. The seeds stay put, like the
    /// guess panel's: a filled cell reads as the user's own edit and
    /// never reseeds away.
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
        // A looked-up release brings sort names with it, so the toggle
        // comes on rather than landing a value in a folded-away row.
        if fills_sort_field(values) {
            self.sort_fields = true;
        }
        // The same rule where the values went into the grid: a sort
        // column nobody asked for comes on, since the fill is already
        // in its cells.
        if to_cells {
            for label in sort_columns_to_show(values, &self.shown) {
                self.toggle_column(label.into(), cx);
            }
        }
        cx.notify();
    }

    /// Commit the armed fields: each input that drifted from its fill
    /// writes its value to every selected file, diffed per file against
    /// that file's own baseline so unchanged fields never rewrite. The
    /// commits run through the writer's atomic layer off the UI thread;
    /// success applies the batch to the catalog and closes the window, a
    /// failure keeps the form open with the error inline, the failed
    /// files untouched.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baselines), false, false) = (&self.baselines, self.saving, self.saved) else {
            return;
        };
        let single = self.tracks.len() == 1;
        let mut armed: Vec<(usize, String)> = Vec::new();
        for (i, (_, _, per_track)) in FIELDS.iter().enumerate() {
            // Per-track fields are disabled in a batch; skipping them
            // here keeps a stale fill from ever counting as an edit.
            if *per_track && !single {
                continue;
            }
            let value = self.inputs[i].read(cx).value().to_string();
            // An armed clear counts even when the input matches its fill:
            // the empty box is the whole point, wiping the tag on every
            // file in the batch.
            if value == self.filled[i].as_ref() && !self.cleared[i] {
                continue;
            }
            armed.push((i, value));
        }
        let mut edits = Vec::new();
        for (t, (track, baseline)) in self.tracks.iter().zip(baselines).enumerate() {
            let mut changes = Vec::new();
            for (i, (field, _, _)) in FIELDS.iter().enumerate() {
                // A form edit is the newest typing and wins its field;
                // otherwise the track's own cell supplies the value once
                // the table exists. A field neither has touched
                // contributes nothing.
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
            // The additional rows, diffed per file like the fields: an
            // armed value from the shared input is the newest typing
            // and stamps the batch, otherwise the file's own cell
            // supplies it once the table exists, a removal or an
            // emptied value drops the key from the files that have it,
            // and a file whose read failed stays untouched, since
            // there's nothing safe to diff it against.
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
                            // A binary row edits nowhere, and an
                            // authored row has no cells at all.
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
                // The sub is paired with the edit: a writer::Edit names a
                // file, and one file can be a dozen cue tracks.
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
            // Note the whole batch before any of it is written, so the watch
            // events these writes trigger are suppressed rather than
            // reindexed. One call up front instead of one per file: the
            // suppression window is seconds long and a batch lands well
            // inside it, and a per-file note would put a main-thread round
            // trip in front of every commit. The apply_edits at the end
            // notes them again for anything still in flight.
            if library
                .update(cx, |library, _| {
                    library.note_self_write(edits.iter().map(|(edit, _)| edit.path.clone()))
                })
                .is_err()
            {
                return;
            }
            // The files are independent and a commit spends most of itself
            // waiting on the disk, so they're written by a small pool over a
            // shared queue rather than one at a time: a batch used to cost
            // the sum of its files. Results come back over the channel as
            // they land, so the count still moves a file at a time and a
            // slow file holds up nothing but its own worker. Capped like the
            // convert and analysis pools, since this is one drive.
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
                            // Through the key: a cue track's edit stays in
                            // the library, since its image belongs to the
                            // whole disc.
                            let result =
                                writer::commit_key(&edit.path, sub, &edit.changes, &edit.pictures);
                            // A closed window drops the receiver: stop
                            // rather than write on into nothing.
                            if tx.send((ix, edit, sub, result)).await.is_err() {
                                break;
                            }
                        }
                    })
                })
                .collect();
            // The loop below owns the last sender; without this the recv
            // never sees the queue run dry.
            drop(tx);
            let mut committed: Vec<Edit> = Vec::new();
            let mut committed_subs: Vec<u16> = Vec::new();
            let mut failures = 0usize;
            // Kept with the index it came in at, so the file the error names
            // is the first one in the list rather than whichever worker
            // happened to fail first.
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
                // A closed window (the user cancelled) drops the handle;
                // stop rather than keep writing into nothing. The workers
                // go with it, so nothing that hasn't started gets written
                // and the commits already running finish on their own.
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
                // A written file's baseline follows the write, so a retry
                // after a partial failure diffs against what's on disk
                // now instead of re-committing the files that succeeded.
                for edit in &committed {
                    let Some(ix) = this.tracks.iter().position(|t| t.path == edit.path) else {
                        continue;
                    };
                    for change in &edit.changes {
                        // An additional change squares its own baseline;
                        // a set replaced every carrier of the key, so
                        // the one written value stands in for them all.
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

    /// The tags no field addresses, editable under their own fold: TXXX
    /// descriptions, the keys lofty maps that the form has no row for,
    /// the binary frames named by size, and the ones the user adds
    /// here. A text value edits in place and arms like a field, the
    /// remove toggle arms the key to leave every carrier on save, and a
    /// binary payload only removes. The header is hand-rolled rather
    /// than [`section`]'s because the count moves with the selection
    /// and that one takes a static label.
    ///
    /// It draws as soon as the reads are in, empty list or not: the add
    /// button lives in its header, and a selection carrying no
    /// additional tags is exactly the one that needs a way to add the
    /// first.
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
            // A row read off a file spells its own key and shows it; an
            // authored one types it, in the slot the label occupies, so
            // the two kinds line up down the list.
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
                    // A file can carry a tag whose name folds to a field's
                    // label without being the tag that field writes, and
                    // the row edits it under the key the file spells. The
                    // note is there so the collision reads as one, instead
                    // of two rows quietly holding the same-looking name.
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
                    // A key a field already owns would edit that field's
                    // tag from here, so the row says which field it
                    // collides with. An authored one saves nothing on top
                    // of that; a row off a file still saves, under the key
                    // the file spells.
                    .when_some(conflict, |d, note| {
                        d.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::tone_warn())
                                .child(note),
                        )
                    })
                    // A key only some of the selection has says so;
                    // one they all have needs no note.
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
                    // The arm toggle, the clear-all chip's language: a
                    // click arms the removal, another takes it back.
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
                    ),
            );
        }
        // Under the table the page never scrolls, so a long list caps
        // and scrolls itself; the form page already scrolls whole.
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
                        // The fold's own hit area stops at the label, so
                        // the add button beside it doesn't close the list
                        // it just added a row to.
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
                        // In the header rather than under the list, so
                        // it holds still as rows are added.
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

    /// The tags section: the shared form, or in table mode the per-track
    /// grid. The lookup is placed beside the heading's name, the mode
    /// toggle and the guess panel at its right edge, since each is about
    /// what the section shows; save and cancel belong to the window and
    /// are in its footer. The table's columns pick through a right click
    /// on their headers, the library table's convention.
    fn tags_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        // The online lookup is the form's alone, single-track only: the
        // compare matches on one track's tags, so a batch has no one
        // query, and in the table every row has its own. Gated on
        // the provider toggle like the metadata panel's.
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
            // The sheet's four sort rows, folded away by default. The
            // table has its own answer in the column menu, so the
            // toggle only draws where it governs something.
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
            // A single file edits in the form alone, so its way into the
            // file manager is here instead of on a table row.
            .when(single, |d| {
                let path = self.tracks[0].path.clone();
                d.child(settings_ui::small_button(
                    rox_i18n::t!("tags-editor-reveal"),
                    icons::FOLDER,
                    false,
                    move |_, _, cx| cx.reveal_path(&path),
                ))
            })
            // A single track fits the form; the table is the batch's
            // per-track view, so only a batch offers the swap.
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
        // The fields and the grid lock while a commit is in flight: a
        // transparent occluder over them swallows clicks and keystrokes
        // so nothing edits out from under the write. Cancel is outside
        // it, down in the footer.
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

    /// Whether a save can run as it stands. A commit diffs each file
    /// against its baseline, so there's nothing safe to write until the
    /// baselines arrive, and a commit already in flight owns the files.
    fn savable(&self) -> bool {
        !self.saving && self.baselines.is_some()
    }

    /// The window's own actions: the save, the way out, and what's
    /// holding the save back when something is. It's on the root
    /// rather than either page, so the buttons keep their place when the
    /// form and the table swap.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let hint: gpui::AnyElement = if self.saving {
            // A commit runs off the UI thread, so say it plainly. The
            // count names how far a slow batch has got instead of
            // freezing on a mute spinner.
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
            // A format the writer has no path for isn't a broken file,
            // so it gets its own line rather than showing the parse error
            // of the read that never happened.
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
                    // Cancel stays live through a save: a slow or wedged
                    // commit needs a way out, and the atomic writer leaves
                    // every original intact whether the batch finished or
                    // not.
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

    /// The shared form: one bare field per row, no input chrome, the
    /// sheet look. Per-track fields have no single form value in a
    /// batch, so they read as plain text and the table edits them. The
    /// four sort rows only draw while the header's toggle is on, and
    /// the label column is sized to the widest label left after that,
    /// so nothing wraps under itself.
    fn form_body(&self, cx: &mut Context<Self>) -> Div {
        let single = self.tracks.len() == 1;
        let shown = form_fields(self.sort_fields);
        let label_w = px(label_column_w(&shown));
        let rows = shown.iter().copied().map(|i| {
            let (field_def, label, per_track) = &FIELDS[i];
            // A mixed batch field can be wiped across every file: its
            // box is empty over the placeholder, so typing can only add
            // a value, never say "clear it everywhere". The toggle does.
            let clearable = !single && !per_track && self.mixed.get(i).copied().unwrap_or(false);
            let cleared = self.cleared.get(i).copied().unwrap_or(false);
            let field: gpui::AnyElement = if *per_track && !single {
                let value = self.inputs[i].read(cx).value();
                let (text, faded) = if self.mixed.get(i).copied().unwrap_or(false) {
                    (rox_i18n::t!("tags-editor-multiple-values"), true)
                } else if value.is_empty() {
                    (SharedString::from("-"), true)
                } else {
                    (value, false)
                };
                div()
                    .when(faded, |d| d.text_color(palette::text_muted()))
                    .child(text)
                    .into_any_element()
            } else if *field_def == Field::Rating && rating_style() == RatingStyle::Stars {
                // Star style rates by click alone, the library cells'
                // face; the numeric style falls through to the plain
                // input below, where 0-10 types exactly.
                rating_field(&self.inputs[i], cx).into_any_element()
            } else {
                // Tab out of a field takes its open suggestion along
                // the way; the move itself is the stock next stop,
                // which already runs down the form.
                let input = self.inputs[i].clone();
                div()
                    .key_context("TagField")
                    .on_action({
                        let input = input.clone();
                        move |_: &FieldTab, window, cx| {
                            take_suggestion(&input, window, cx);
                            window.focus_next();
                            // Same propagation hazard as
                            // accept_then_focus: without this the
                            // root's tab binding moves a second time.
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
            div()
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
                        // Title cased the way the table's headings
                        // are. FIELDS keeps its lowercase literals
                        // because the column sets and the tests
                        // match on them, so capitalising is the
                        // drawing's business rather than the
                        // table's.
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
        });
        div().flex().flex_col().gap(px(2.)).children(rows)
    }

    /// The table over the grid: resizable, sortable columns like the
    /// library's list, every field editable per track. Tab moves down
    /// each column, top to bottom.
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

/// The grid's delegate: the cells are the editor's own inputs, shared by
/// entity, so the table shows exactly the state save reads. `names` holds
/// the file column's read-only inputs, parallel to `cells`, and `order` is
/// the sort permutation from display row to track index. The editor is
/// held weakly so a row's own lookup can reach it from the cell.
struct CellGrid {
    columns: Vec<Column>,
    cells: Vec<Vec<Entity<InputState>>>,
    /// The additional tags that can hold a column, in the order the
    /// section lists them. A column addresses one of these by its place
    /// here, which is also where its cells sit in `tag_cells`.
    tags: Vec<TagColumn>,
    tag_cells: Vec<Vec<TagCell>>,
    names: Vec<Entity<InputState>>,
    order: Vec<usize>,
    editor: WeakEntity<TagEditor>,
}

/// The file column, then one per field, then one per additional tag:
/// name columns wide, numeric ones narrow, all resizable and sortable
/// like the library's list. `saved` overrides the defaults with the
/// last editor's widths, one slot per column in the fixed order. Those
/// widths are positional, so a set written before a column existed
/// falls back to the defaults rather than being applied to the wrong
/// columns. `tag_widths` is keyed instead, since the tag set changes
/// with the selection and a slot would land its width on whichever tag
/// happened to sort into that place.
///
/// The columns nobody asked for drop out after the widths resolve, so a
/// column keeps its width across a toggle. A set that would empty the
/// table is ignored, since an empty table has no header to bring one
/// back from.
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
        // Only the fixed columns: falling back to every tag as well
        // would answer an empty table with a wall of them.
        columns.into_iter().take(LEAD + FIELDS.len()).collect()
    } else {
        picked
    }
}

impl CellGrid {
    /// What a column edits. By key rather than position: the columns
    /// nobody asked for leave the display order sparse.
    fn kind(&self, col_ix: usize) -> Option<ColumnKind> {
        column_kind(self.columns[col_ix].key.as_ref(), &self.tags)
    }

    /// The columns holding a focusable cell, in display order: the tab
    /// order runs down each of these in turn. A column that isn't on
    /// screen keeps its cells and their edits, but focusing one would
    /// put the cursor somewhere the table doesn't draw, and the file
    /// column, a star rating, and a binary tag hold no input at all.
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

    /// One track's cell under a column, when the column holds an input.
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

    /// The file column's cell: the name on a bare disabled input so its
    /// text selects and copies the way the library's lines do (the
    /// component only gates typing on disabled, never selection), then
    /// the row's way into the file manager and its own lookup. A lookup
    /// matches one file's tags against a release, so once the grid is
    /// showing many files the row is the only honest place for it.
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
            // Gated on the provider toggle like the header's, which the
            // form still shows for a single track.
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

    /// The header cell: the stock label plus a right-click menu that
    /// toggles the shown columns in place, the library table's
    /// convention. The additional tags come after the fields under
    /// their own heading, so a selection carrying a screenful of stray
    /// keys reads as two groups rather than one long list.
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

    /// Sort the rows by the column's current cell values, numerics by
    /// their leading digits the way the scanner reads them. The cells
    /// travel with their track, so no edit is lost to a re-order.
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
        // The file column sorts on its name; the rest on their cells.
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
        // Star-style rating cells hold no focusable input: they render
        // the click control and stay outside the tab order. The numeric
        // style keeps them as plain 0-10 inputs in the order built below.
        let stars = rating_style() == RatingStyle::Stars;
        let kind = self.kind(col_ix);
        match kind {
            // The file column edits nothing and stays out of the tab
            // order; so does a column a hand-edited settings file named
            // and nothing here answers to.
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
            // A binary payload's size, read-only: the form never edited
            // bytes and a column doesn't change that.
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
        // The neighbors down and up the column, wrapping into the next
        // and previous column at the ends. The stops are the columns
        // holding an input, so a rating under stars, a binary tag, and
        // anything toggled off are all already out.
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
        // Tab moves down the column instead of across the row: the
        // editor's own binding catches it here, deeper than the window
        // root's, and moves to the neighbor we compute instead of the
        // paint-order stop.
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

/// The selection's additional tags as one editable list: every key any
/// file has, ordered by how many have it so the shared ones lead,
/// alphabetical inside a tie so the order holds still across opens. The
/// table's tag columns run in this same order. A text key every carrier
/// agrees on fills its input with the value; disagreeing carriers leave
/// it empty over the mixed placeholder, the form's convention. The
/// initial snapshot reads back off the input, so an untouched row can
/// never drift from what it filled with.
fn build_additional(
    reads: &[FileRead],
    window: &mut Window,
    cx: &mut Context<TagEditor>,
) -> AdditionalTags {
    // (key, one value per sighting, the file indices that carried it).
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
            // Bytes never edit: the row shows its size and only removes.
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
            }
        })
        .collect::<Vec<_>>();
    AdditionalTags {
        // Every row here came off a file, so every one can hold a
        // column. The authored rows the add button appends land after
        // these and past the count.
        columns: rows.len(),
        rows,
        failed,
        files: reads.len(),
    }
}

/// A tag value as one row of it: newlines and control bytes flattened to
/// spaces, and a long value cut where reading it stops being the point.
/// A lyric sheet or an embedded blob of json is a tag like any other and
/// still has to fit a row.
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

/// A value's leading digits, the scanner's read of a numeric tag.
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
        // The table scrolls its own rows inside a fixed page, the tag
        // list capped under it; the form page scrolls whole under the
        // shared scrollbar.
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
            // The backdrop paints first, under the page; without it
            // translucent surfaces would sink into the window's own
            // black instead of the playing track's art.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div().flex_1().min_h_0().flex().flex_row().child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        // The page's own surface, a second elevated layer over
                        // the window's, the same as the settings page. The
                        // backdrop reads through two layers, and the
                        // footer stays outside it to render a step darker.
                        .bg(palette::bg_elevated())
                        .child(page)
                        // Fades out when idle, same as the panels.
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
        baseline_value, change_for, column_keys, column_kind, column_rank, column_shown,
        default_widths, file_tag_key, fills_sort_field, fold_cell, fold_tag_intents, form_fields,
        grid_columns, label_column_w, placed_widths, shared_value, sort_columns_to_show,
        sort_field, tag_change_for, tag_key_of, title_case, ColumnKind, TagColumn, TagIntent,
        FIELDS, LABEL_MIN_W, LEAD, OPT_IN_COLUMNS,
    };
    use rox_library::writer::{Field, UnknownValue};
    use std::collections::{BTreeMap, HashSet};

    /// One file's baseline as the writer's read hands it over.
    fn baseline(pairs: &[(Field, &str)]) -> Vec<(Field, String)> {
        pairs
            .iter()
            .map(|(field, value)| (field.clone(), (*value).to_string()))
            .collect()
    }

    /// One file's additional tags as the writer's read hands them over.
    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, UnknownValue)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), UnknownValue::Text((*value).to_string())))
            .collect()
    }

    /// The tag columns a selection carrying these keys would offer.
    fn tag_columns(keys: &[&str]) -> Vec<TagColumn> {
        keys.iter()
            .map(|key| TagColumn {
                key: (*key).to_string(),
                name: (*key).to_string().into(),
                text: true,
            })
            .collect()
    }

    /// A set from a list, for the two column sets.
    fn set(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    /// Every sort field sits directly under the field it sorts, and
    /// carries that field's per-track bool. The order is what the form
    /// rows, the table columns, and the saved width slots all read.
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

    /// The batch case the sort names exist for: one file already carries
    /// an artist sort name and the other doesn't, so the form shows the
    /// field as mixed, and arming it writes to both files, since neither
    /// baseline matches the typed value.
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
        // The artist itself agrees, so it fills with the shared value.
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

    /// A field left alone contributes nothing, whether the file carries
    /// it or not; an emptied one drops the tag. That's what keeps a save
    /// from rewriting the files it never touched.
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

    /// Every column resolves to exactly one kind, and a tag resolves to
    /// its own slot rather than falling through to the file column. The
    /// whole table keys off this: sorting, the cells, and the tab order
    /// all match on the answer.
    #[test]
    fn a_column_resolves_to_one_kind() {
        let tags = tag_columns(&["MOOD", "ISRC"]);
        assert!(column_kind("file", &tags) == Some(ColumnKind::File));
        assert!(column_kind("album artist", &tags) == Some(ColumnKind::Field(4)));
        assert!(column_kind("tag:MOOD", &tags) == Some(ColumnKind::Tag(0)));
        assert!(column_kind("tag:ISRC", &tags) == Some(ColumnKind::Tag(1)));
        // A tag the current selection doesn't carry, and a key nothing
        // answers to: neither is the file column.
        assert!(column_kind("tag:GONE", &tags).is_none());
        assert!(column_kind("nonsense", &tags).is_none());
        // A stray tag spelled like a field is still a tag, which is
        // what the prefix exists for.
        let shadow = tag_columns(&["album"]);
        assert!(column_kind("tag:album", &shadow) == Some(ColumnKind::Tag(0)));
        assert!(column_kind("album", &shadow) == Some(ColumnKind::Field(6)));
    }

    /// The tag columns sit after every fixed column, in the order the
    /// section lists them, so a column toggled off and back lands where
    /// it was rather than at the end.
    #[test]
    fn tag_columns_rank_after_the_fields() {
        let tags = tag_columns(&["MOOD", "ISRC"]);
        assert!(column_rank("file", &tags) == Some(0));
        assert!(column_rank("title", &tags) == Some(LEAD));
        assert!(column_rank("tag:MOOD", &tags) == Some(LEAD + FIELDS.len()));
        assert!(column_rank("tag:ISRC", &tags) == Some(LEAD + FIELDS.len() + 1));
    }

    /// Fields show unless they're hidden; the sort names and the tags
    /// show only when they're asked for. A fresh editor has both sets
    /// empty, which is what puts the four sort columns off the table
    /// without hiding anything the user picked.
    #[test]
    fn sort_and_tag_columns_start_off() {
        let (hidden, shown) = (set(&[]), set(&[]));
        for (_, label, _) in FIELDS {
            let asked = OPT_IN_COLUMNS.contains(label);
            assert!(column_shown(label, &hidden, &shown) != asked, "{label}");
        }
        assert!(column_shown("file", &hidden, &shown));
        assert!(!column_shown("tag:MOOD", &hidden, &shown));

        // And a pick, either way, holds.
        let shown = set(&["album sort", "tag:MOOD"]);
        let hidden = set(&["genre"]);
        assert!(column_shown("album sort", &hidden, &shown));
        assert!(column_shown("tag:MOOD", &hidden, &shown));
        assert!(!column_shown("title sort", &hidden, &shown));
        assert!(!column_shown("genre", &hidden, &shown));
    }

    /// Widths written before the sort-name columns existed land on the
    /// columns they were measured for, rather than resetting the table
    /// once for everyone who ever dragged a divider.
    #[test]
    fn widths_from_before_the_sort_columns_are_placed() {
        let defaults = default_widths();
        // What the old build wrote: the same order minus the four sort
        // names, with each slot given a value that names itself.
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
        // A set this build's layout already fits comes back untouched,
        // and any other length is nobody's layout.
        assert!(placed_widths(&defaults).as_deref() == Some(defaults.as_slice()));
        assert!(placed_widths(&[]).is_none());
        assert!(placed_widths(&old[..old.len() - 1]).is_none());

        // Through the table: the title's old width follows it into the
        // new order, and the sort column beside it opens at its default.
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

    /// A tag column's width is kept by key, so it survives editing a
    /// selection that carries a different set of tags in between, and
    /// the fixed columns' positional slots are untouched by any of it.
    #[test]
    fn tag_widths_are_kept_by_key_not_by_slot() {
        let saved = default_widths();
        let mut widths = BTreeMap::new();
        widths.insert("MOOD".to_string(), 300.);
        let shown = set(&["tag:MOOD", "tag:ISRC"]);
        let hidden = set(&[]);

        // A selection carrying both, and then one carrying only the
        // second: MOOD's width is waiting either way, and ISRC, which
        // nothing ever sized, opens at the default.
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
        // The fixed slots read the same in both, tag set or no tag set.
        for columns in [&both, &one] {
            assert!(width(columns, "file") == saved[0]);
            assert!(width(columns, "title") == saved[LEAD]);
        }
    }

    /// A key a field already owns is refused rather than written from
    /// two places, whatever way it's spelled; anything else is the
    /// user's to name.
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
        // A blank key addresses nothing, and the writer would take it
        // seriously, so it never reaches a change.
        assert!(tag_key_of("").is_none());
        assert!(tag_key_of("   ").is_none());
    }

    /// The refusals are the authored row's alone. A file that carries a
    /// TXXX called ALBUMARTISTSORT, or one whose description has a space
    /// on it, gets a row that removes and rewrites under that exact key,
    /// since it's the only place those tags can be reached.
    #[test]
    fn a_file_spells_its_own_key() {
        for key in ["ALBUMARTISTSORT", "DATE", " MOOD "] {
            assert!(tag_key_of(key) != Some(key.to_string()), "{key}");
            assert!(file_tag_key(key) == Some(key.to_string()), "{key}");
        }
        assert!(file_tag_key("").is_none());

        // And the change carries it untouched, so the baseline lookup and
        // the writer's verify both find the tag they mean.
        let file = tags(&[(" MOOD ", "calm")]);
        let key = file_tag_key(" MOOD ").expect("a key off a file");
        let change = tag_change_for(&key, &TagIntent::Drop, &file).expect("the row removes");
        assert!(change.field == Field::Unknown(" MOOD ".to_string()));
        assert!(change.value.is_none());
        // A trimmed key would have addressed a tag the file doesn't hold,
        // and the removal would have gone nowhere.
        assert!(tag_change_for("MOOD", &TagIntent::Drop, &file).is_none());
    }

    /// The add button's case: a key none of the files carry, typed into
    /// an authored row, writes exactly one change to every file in the
    /// selection.
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

    /// An authored row landing on a key that's already in the list
    /// folds into one change rather than two, since the writer applies
    /// changes in order and the second would quietly win anyway.
    #[test]
    fn two_rows_on_one_key_fold_into_one() {
        let read = ("MOOD".to_string(), TagIntent::Keep);
        let authored = ("MOOD".to_string(), TagIntent::Set("restless".to_string()));
        let folded = fold_tag_intents(vec![read.clone(), authored.clone()]);
        assert!(folded == vec![("MOOD".to_string(), TagIntent::Set("restless".to_string()))]);

        // And the other way round: a row asking for nothing never
        // erases one that asks for something.
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

    /// A row left alone costs nothing, an armed removal only touches
    /// the files that carry the key, and a value a file already spells
    /// that way never rewrites it.
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

    /// ADR 18's last-edit-wins rule, which fields and additional tags
    /// both read from here. Entering the table folds a drifted form
    /// value into every untouched cell; a cell the user already moved
    /// keeps their value; a quiet form leaves the file's own baseline
    /// standing.
    #[test]
    fn a_drifted_form_folds_into_untouched_cells_only() {
        // Untouched: the cell is still on what it last seeded from.
        let folded = fold_cell("Lemon", "Lemon", "Lemon", Some("レモン"));
        assert!(folded == Some("レモン".into()));
        // Moved: the cell is the newest typing for that file and stands.
        assert!(fold_cell("Kenshi", "Lemon", "Lemon", Some("レモン")).is_none());
        // A quiet form on the first build seeds the file's own value.
        assert!(fold_cell("", "", "Lemon", None) == Some("Lemon".into()));
        // A cell sitting on a seed an earlier fold brought in would be
        // pushed back to the file's baseline by this rule alone, which
        // is why the column above it only re-seeds on the first build
        // or under live form drift.
        assert!(fold_cell("レモン", "レモン", "Lemon", None) == Some("Lemon".into()));
    }

    /// Every label the sheet draws reads as a label: each word starts
    /// capitalized and nothing doubles a space along the way. FIELDS
    /// keeps its lowercase literals, since the column sets and the
    /// tests match on them, so this is the only place the difference
    /// between the two shows up.
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
            // Only the case moves; the words themselves stay put.
            assert!(cased.to_lowercase() == *label, "{cased}");
        }
        assert!(title_case("album artist sort") == "Album Artist Sort");
    }

    /// The toggle folds away exactly the four sort rows and nothing
    /// else, and turning it on puts every field back in FIELDS order,
    /// so a row's index still names its input, fill, and mixed flag.
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

    /// A looked-up release brings sort names with it. The fill lands in
    /// the inputs either way, so a non-empty one opens the rows rather
    /// than sitting where nobody can see it; an empty one, or a match
    /// with no sort names at all, leaves the toggle alone.
    #[test]
    fn a_filled_sort_name_opens_the_rows() {
        let named = [
            (Field::Artist, "米津玄師".to_string()),
            (Field::ArtistSort, "Yonezu, Kenshi".to_string()),
        ];
        assert!(fills_sort_field(&named));

        let plain = [(Field::Artist, "米津玄師".to_string())];
        assert!(!fills_sort_field(&plain));
        // A match that answers with nothing for the sort name doesn't
        // count as one: the row would open empty.
        let blank = [(Field::AlbumArtistSort, "   ".to_string())];
        assert!(!fills_sort_field(&blank));
    }

    /// The table's half of the same rule: a fill goes into the named
    /// track's cells whether the column is up or not, so the sort
    /// columns it wrote to come on. Only those, only when the value is
    /// worth showing, and never one that's already up.
    #[test]
    fn a_filled_sort_name_opens_its_column() {
        let filled = [
            (Field::Artist, "米津玄師".to_string()),
            (Field::ArtistSort, "Yonezu, Kenshi".to_string()),
            (Field::AlbumSort, String::new()),
        ];
        assert!(sort_columns_to_show(&filled, &set(&[])) == vec!["artist sort"]);
        // Already asked for, so there's nothing to turn on.
        assert!(sort_columns_to_show(&filled, &set(&["artist sort"])).is_empty());
        // Two at once, in FIELDS order.
        let both = [
            (Field::TitleSort, "Lemon".to_string()),
            (Field::AlbumArtistSort, "Yonezu, Kenshi".to_string()),
        ];
        assert!(sort_columns_to_show(&both, &set(&[])) == vec!["title sort", "album artist sort"]);
        // A fill with no sort names touches no column.
        let plain = [(Field::Album, "レモン".to_string())];
        assert!(sort_columns_to_show(&plain, &set(&[])).is_empty());
    }

    /// The label column fits the widest label it draws, which is what
    /// keeps "Album Artist Sort" on one line. Folding the sort rows
    /// away narrows it, and it never drops under the floor.
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
        // The sort names are the long ones, so the open sheet's column
        // is the wider of the two.
        assert!(label_column_w(&form_fields(true)) > label_column_w(&form_fields(false)));
        // And an empty sheet still draws its floor rather than nothing.
        assert!(label_column_w(&[]) == LABEL_MIN_W);
    }
}
