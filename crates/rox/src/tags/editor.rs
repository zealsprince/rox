//! The tag editor window: one OS window opened on a selection - albums
//! picked in the grid, tracks picked in the library - rather than a panel,
//! since editing wants room and a plain close-without-saving story. One
//! shared field form sits over the selection's track list: a field every
//! file agrees on shows its value, differing values show empty over a
//! "multiple values" placeholder, and only the fields the user moves
//! write anything. Table mode swaps the form for one row of cells per
//! track, where the per-track fields a batch form has to lock stay
//! editable and tab walks the grid. The name fields suggest the
//! library's own values as they are typed. Baselines come off each file
//! through the writer's read,
//! the metadata panel's convention, so every save diffs per file against
//! what that file actually carries and commits through the atomic layer.
//! A successful save lands in the catalog in one batch, then re-reads the
//! written files so their rows converge with what is on disk - duration and
//! the rest the form never named included.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, div, prelude::*, px, size, svg, App, Bounds, Context, Div, Entity, FocusHandle,
    Focusable as _, Global, KeyBinding, MouseButton, ScrollHandle, SharedString, Subscription,
    WeakEntity, Window, WindowHandle,
};
use gpui_component::button::Button;
use gpui_component::input::{Enter, Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Root, Sizable, Size};

use rox_core::fmt::fmt_ms;
use rox_library::cue::TrackKey;
use rox_library::projection::Projection;
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

/// The form's fields in sheet order: the label each row wears, and
/// whether the field is per-track by nature. Per-track fields only edit
/// while a single track is selected; a batch would stamp one title or
/// track number over every file.
const FIELDS: &[(Field, &str, bool)] = &[
    (Field::Title, "title", true),
    (Field::Artist, "artist", false),
    (Field::AlbumArtist, "album artist", false),
    (Field::Album, "album", false),
    (Field::Genre, "genre", false),
    (Field::Year, "year", false),
    (Field::TrackNo, "track", true),
    (Field::DiscNo, "disc", true),
    (Field::Comment, "comment", false),
    // Shared on purpose: rating an album's files in one stroke is the
    // batch case the user asked for. The value speaks the writer's 0-10
    // number, half points included.
    (Field::Rating, "rating", false),
];

/// How many display columns lead the table ahead of the editable
/// [`FIELDS`] grid in the full column order. The file column is one of
/// these: it carries no input and no field, and a save never sees it.
/// The settings file's width slots are positional over this full order,
/// hidden columns included, so a width survives its column being toggled
/// away and back.
const LEAD: usize = 1;

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

/// A key's slot in the full column order, the position its width lives
/// at in the settings file whether the column shows or not.
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

/// A path as the row shows it: the file name alone, the whole path when
/// there is no name to take.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The rating inputs' empty-state hint, the one field whose scale is not
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
    // The input's entity id keys the hover preview; unlike a track id it
    // is unique per editor row.
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
/// Enter sits on the window root instead, so it saves from a field, a
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
/// when one is up and does nothing when it is not. Dispatching the input's
/// Enter action instead would, with no menu open, emit PressEnter, which
/// the save subscription reads as a save and closes the window - that is
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
/// focuses that window instead of stacking a twin - an edit in progress
/// is not worth losing.
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
                "rox - Tag Editor",
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| TagEditor::new(state, ids, window, cx)),
            )
        },
        cx,
    );
}

/// One selected track as the list shows it, resolved at open; the path is
/// what the baselines read and the commits write, and the sub says which row
/// of it they belong to when the file is a cue image.
struct TrackRow {
    path: PathBuf,
    sub: u16,
    title: SharedString,
    /// The row's display line (title, artist when tagged) in a read-only
    /// input, so its text selects and copies into the fields - the way
    /// into retagging files whose only metadata is their name.
    line: Entity<InputState>,
    duration_ms: u32,
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

/// The selection's tags that no field addresses, unioned into one
/// read-only list.
struct UnknownTags {
    rows: Vec<UnknownRow>,
    /// How many files' unknown reads failed. The list is short by that
    /// many, so the section says so rather than passing for complete.
    failed: usize,
    /// How many files the union covers, for the per-row "3 of 7".
    files: usize,
}

/// One key in that list: what it's called, what the files carry under
/// it, and how many of them do.
struct UnknownRow {
    key: SharedString,
    value: SharedString,
    files: usize,
}

pub struct TagEditor {
    library: Entity<Library>,
    tracks: Vec<TrackRow>,
    /// Each file's fields as the writer read them, parallel to `tracks`:
    /// what save diffs against, per file. None until every read lands
    /// (or never, when a file defeats the parser), and save stays inert
    /// without it.
    baselines: Option<Vec<Vec<(Field, String)>>>,
    /// What the form filled each input with once the baselines landed:
    /// the value every file shares, or empty under the mixed
    /// placeholder. A field arms by drifting from this.
    filled: Vec<SharedString>,
    /// Whether each field's files disagreed at the last fill; the
    /// read-only per-track rows say so instead of faking one value.
    mixed: Vec<bool>,
    /// Whether the user armed a batch field to clear across every file.
    /// A mixed field sits empty over its placeholder, so an empty input
    /// alone can't say "wipe this tag on all of them" - this flag does,
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
    /// The column keys toggled off the table, remembered through the
    /// settings file like the widths. A hidden column's cells live on,
    /// so nothing typed there is lost to a toggle.
    hidden: HashSet<String>,
    /// What each cell last seeded from. A cell still on its seed follows
    /// re-seeds (a form edit folding in); one the user moved is theirs.
    seeds: Vec<Vec<SharedString>>,
    /// The projection the suggestion providers share, kept for cells
    /// created after open.
    projection: Option<Arc<Projection>>,
    /// The guess panel is open: a filename pattern with a live preview
    /// of the values it would pull from every track's path.
    guess: bool,
    /// The guess pattern's input, remembered across editors through the
    /// settings file - one library tends to one naming scheme.
    pattern: Entity<InputState>,
    /// The tags no field addresses, read-only under their own fold.
    /// None until the reads land; a file whose unknown read failed only
    /// costs its own rows, never the form.
    unknowns: Option<UnknownTags>,
    /// Whether that fold is open. Closed at open: most files carry a few
    /// of these and some carry a screenful, and none of it is editable.
    unknowns_open: bool,
    /// How many of the selection are in a format the writer has no path
    /// for. Those files say so plainly instead of wearing a parse error
    /// over a dead form.
    unsupported: usize,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// A commit is in flight; the fields lock and the buttons hold still
    /// until it lands.
    saving: bool,
    /// The save already ran and the window is on its way out. One enter
    /// press can reach [`Self::save`] twice - the focused input's own
    /// binding and the window root's, which the input propagates to - and
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
                    Some((
                        v.title.to_owned(),
                        v.artist.to_owned(),
                        v.duration_ms,
                        v.sub,
                    ))
                });
                let (title, artist, duration_ms, sub) = resolved.unwrap_or_else(|| {
                    let title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    (title, String::new(), 0, 0)
                });
                tracks.push((path, sub, title, artist, duration_ms));
            }
            tracks
        };
        let tracks: Vec<TrackRow> = tracks
            .into_iter()
            .map(|(path, sub, title, artist, duration_ms)| {
                let mut line = title.clone();
                if !artist.is_empty() {
                    line.push_str(" - ");
                    line.push_str(&artist);
                }
                let line = cx.new(|cx| InputState::new(window, cx).default_value(line));
                TrackRow {
                    path,
                    sub,
                    title: title.into(),
                    line,
                    duration_ms,
                }
            })
            .collect();
        let inputs: Vec<Entity<InputState>> = FIELDS
            .iter()
            .map(|(field, _, _)| {
                cx.new(|cx| {
                    let mut input =
                        InputState::new(window, cx).placeholder(field_placeholder(field));
                    if *field == Field::Rating {
                        // The scale is not free text; typing anything it
                        // cannot parse never lands in the field.
                        input = input.validate(|s, _| {
                            s.trim().is_empty() || rating::parse_display(s).is_some()
                        });
                    }
                    input.lsp.completion_provider = suggest::provider(projection.as_ref(), field);
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
        // the guesses rather than saving - the preview is right there and
        // an accidental save would close the window.
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
        // A multi-selection opens straight into the table - per-track
        // editing is what it is for; a single track fits the form.
        let table = tracks.len() > 1;
        // The columns the last editor toggled away, pruned of anything
        // that stopped being a column since it was written.
        let hidden: HashSet<String> = Settings::load()
            .windows
            .tag_editor
            .map(|s| s.hidden)
            .unwrap_or_default()
            .into_iter()
            .filter(|key| canonical_ix(key).is_some())
            .collect();
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
            seeds: Vec::new(),
            projection,
            guess: false,
            pattern,
            unknowns: None,
            unknowns_open: false,
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
    /// they all land. One unreadable file blocks the whole save: without
    /// its baseline there is nothing safe to diff that file against. The
    /// read-only tags ride the same hop, one file at a time, so the list
    /// costs nothing extra in wall time and a file that defeats it costs
    /// only its own rows.
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
                this.unknowns = Some(gather_unknowns(&reads));
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

    /// Open or close the read-only tag list.
    fn toggle_unknowns(&mut self, cx: &mut Context<Self>) {
        self.unknowns_open = !self.unknowns_open;
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
            let mut values = baselines.iter().map(|fields| {
                fields
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
            });
            let first = values.next().unwrap_or_default();
            let mixed = values.any(|v| v != first);
            let value = if mixed {
                SharedString::default()
            } else {
                SharedString::from(first.to_owned())
            };
            input.update(cx, |input, cx| {
                if mixed {
                    input.set_placeholder("Multiple values", window, cx);
                }
                input.set_value(value.clone(), window, cx);
            });
            self.filled.push(value);
            self.mixed.push(mixed);
        }
        self.baselines = Some(baselines);
        // A table-first open can only build its cells once the baselines
        // land, so they seed here.
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
        let pattern = self.pattern.read(cx).value().to_string();
        Settings::update(move |s| {
            let state = s.windows.tag_editor.get_or_insert_with(Default::default);
            state.width = frame.size.width.into();
            state.height = frame.size.height.into();
            // A form-only session has no table; keep the saved widths.
            // The shown columns write into their slots in the full order,
            // so a hidden column's width rides along untouched.
            if !columns.is_empty() {
                if state.columns.len() != LEAD + FIELDS.len() {
                    state.columns = default_widths();
                }
                for (key, width) in &columns {
                    if let Some(ix) = canonical_ix(key) {
                        state.columns[ix] = *width;
                    }
                }
            }
            state.hidden = hidden;
            state.pattern = pattern;
        });
    }

    /// The first field the toggles leave on screen, where table focus
    /// lands: the title unless its column is hidden.
    fn first_visible_field(&self) -> usize {
        FIELDS
            .iter()
            .position(|(_, label, _)| !self.hidden.contains(*label))
            .unwrap_or(0)
    }

    /// Show or hide a table column, keeping the rest in place. A shown
    /// column returns to its slot in the field order at its default
    /// width; hiding drops it, and never the last one, since an empty
    /// table has no header to bring one back from.
    fn toggle_column(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(grid) = &self.grid else { return };
        if self.hidden.remove(key) {
            grid.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                let Some(canon) = canonical_ix(key) else {
                    return;
                };
                // The table never reorders columns, so the shown set
                // stays in the field order and the slot count places it.
                let at = delegate
                    .columns
                    .iter()
                    .take_while(|c| canonical_ix(c.key.as_ref()).unwrap_or(usize::MAX) < canon)
                    .count();
                let column = Column::new(key, title_case(key))
                    .width(px(default_widths()[canon]))
                    .sortable();
                delegate.columns.insert(at, column);
                table.refresh(cx);
            });
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
                self.hidden.insert(key.to_string());
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
    /// in flight. A folded-in form edit stops counting as form drift -
    /// the cells carry it from here - and a cell the user already moved
    /// keeps their value.
    fn seed_cells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(baselines) = self.baselines.clone() else {
            return;
        };
        let created = self.cells.is_none();
        if created {
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
                        input.lsp.completion_provider =
                            suggest::provider(self.projection.as_ref(), field);
                        input
                    });
                    // A rating click lands in the cell's input; without
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
            // The file column's names ride bare disabled inputs, the
            // track list's trick for text that has to select and copy.
            // Built with the grid, so a form-only session pays nothing.
            let names: Vec<Entity<InputState>> = self
                .tracks
                .iter()
                .map(|track| {
                    let name = file_name(&track.path);
                    cx.new(|cx| InputState::new(window, cx).default_value(name))
                })
                .collect();
            let saved = Settings::load()
                .windows
                .tag_editor
                .map(|s| s.columns)
                .unwrap_or_default();
            let delegate = CellGrid {
                columns: grid_columns(&saved, &self.hidden),
                cells: cells.clone(),
                names,
                order: (0..cells.len()).collect(),
                editor: cx.entity().downgrade(),
            };
            let grid = cx.new(|cx| TableState::new(delegate, window, cx));
            // The component owns the live column widths; mirror a resize
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
            self.seeds = vec![vec![SharedString::default(); FIELDS.len()]; self.tracks.len()];
        }
        for (i, (field, _, _)) in FIELDS.iter().enumerate() {
            let form_value = self.inputs[i].read(cx).value().to_string();
            let drifted = form_value != self.filled[i].as_ref();
            // Once the grid exists the cells carry the truth: a re-entry
            // only folds in live form drift. Re-seeding a quiet field
            // would push its cells back to the disk baseline, wiping the
            // values an earlier fold-in carried.
            if !created && !drifted {
                self.cleared[i] = false;
                continue;
            }
            for (t, baseline) in baselines.iter().enumerate() {
                let base = baseline
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");
                let target: SharedString = if drifted {
                    form_value.clone().into()
                } else {
                    base.to_owned().into()
                };
                let cell = self.cells.as_ref().unwrap()[t][i].clone();
                let current = cell.read(cx).value().clone();
                if current != self.seeds[t][i] {
                    continue;
                }
                if current != target {
                    let value = target.clone();
                    cell.update(cx, |cell, cx| cell.set_value(value, window, cx));
                }
                self.seeds[t][i] = target;
            }
            if drifted {
                self.filled[i] = form_value.into();
            }
            // The cells carry the truth from here, so a pending clear-all
            // the form carried is off - left armed, save would wipe the tag
            // on every file while the table showed the original values.
            self.cleared[i] = false;
        }
    }

    /// Leave table mode: the form re-reads the cells - a field the rows
    /// agree on shows the value, a split one goes back to empty over the
    /// mixed placeholder - and the fill snapshot follows, so only typing
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
                let placeholder = if mixed {
                    "Multiple values"
                } else {
                    field_placeholder(&FIELDS[i].0)
                };
                input.set_placeholder(placeholder, window, cx);
                input.set_value(value.clone(), window, cx);
            });
            self.filled[i] = value;
            self.mixed[i] = mixed;
            // The table re-read is a fresh baseline, so any pending
            // clear-all the form carried is off.
            self.cleared[i] = false;
        }
    }

    /// Toggle a batch field's clear-all arm: on, the field wipes its tag
    /// across every file in the selection on save; off, it goes back to
    /// leaving the split values alone. Only the shared form's mixed fields
    /// get this - a single track just empties its box.
    fn toggle_clear(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.cleared.get(i).copied().unwrap_or(false);
        self.cleared[i] = on;
        self.inputs[i].update(cx, |input, cx| {
            if on {
                input.set_value("", window, cx);
                input.set_placeholder("Clear on save", window, cx);
            } else {
                input.set_placeholder("Multiple values", window, cx);
            }
        });
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

    /// Write the pattern's matches into the editor: per-track values land
    /// in the table's cells (switching to table mode to show them), a
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
                                .child(div().text_color(palette::text_muted()).child(label))
                                .child(SharedString::from(value.clone()))
                        }))
                        .into_any_element(),
                    None => div()
                        .text_color(palette::text_muted())
                        .child("no match")
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
            None => format!("{hits} of {} match", self.tracks.len()).into(),
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
                            .child("pattern"),
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
                    .child(format!(
                        "{}; / matches the folder above, %skip% discards",
                        guess::PLACEHOLDERS.join(" ")
                    )),
            )
            .children(rows)
            .child(div().text_xs().text_color(palette::text_muted()).map(|d| {
                if folded > 0 {
                    d.child(format!("{status}, {folded} more not shown"))
                } else {
                    d.child(status)
                }
            }))
    }

    /// Open the metadata compare on one edited track. The window
    /// searches, ranks matches, and on apply calls back into
    /// [`Self::fill_fields`] rather than writing, so this editor stays the
    /// one writer. A lookup is one track's by nature: the form's header
    /// button carries it for a single track, the table's rows one each.
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
    /// save writes it and nothing lands until the user saves. Fields the
    /// match does not carry are left untouched. The compare calls this on
    /// its own apply, on this editor's window, naming the track it ran on.
    ///
    /// Where the values land is where the user can see them: the named
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
        cx.notify();
    }

    /// Commit the armed fields: each input that drifted from its fill
    /// writes its value to every selected file, diffed per file against
    /// that file's own baseline so unchanged fields never rewrite. The
    /// commits run through the writer's atomic layer off the UI thread;
    /// success lands the batch in the catalog and closes the window, a
    /// failure keeps the form open with the error inline, the failed
    /// files untouched.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baselines), false, false) = (&self.baselines, self.saving, self.saved) else {
            return;
        };
        let single = self.tracks.len() == 1;
        let mut armed: Vec<(usize, String)> = Vec::new();
        for (i, (_, _, per_track)) in FIELDS.iter().enumerate() {
            // Per-track fields sit disabled in a batch; skipping them
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
                // otherwise the track's own cell speaks once the table
                // exists. A field neither has touched says nothing.
                let value = match armed.iter().find(|(armed_ix, _)| *armed_ix == i) {
                    Some((_, value)) => value.clone(),
                    None => match &self.cells {
                        Some(cells) => cells[t][i].read(cx).value().to_string(),
                        None => continue,
                    },
                };
                let original = baseline
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");
                if value == original {
                    continue;
                }
                changes.push(Change {
                    field: field.clone(),
                    value: (!value.is_empty()).then_some(value),
                });
            }
            if !changes.is_empty() {
                // The sub rides beside the edit: a writer::Edit names a file,
                // and one file can be a dozen cue tracks.
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
            // One file per background hop, not the whole batch behind a
            // single await: the count moves as each lands, a slow file is
            // visibly the one holding things up, and a cancel that closes
            // the window ends the loop instead of grinding on unseen.
            let mut committed: Vec<Edit> = Vec::new();
            let mut committed_subs: Vec<u16> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for (edit, sub) in edits {
                // Note the write before it lands so the watch batch it
                // triggers is suppressed, not reindexed. The apply_edits at
                // the end notes too, but by then the suppression window has
                // long passed for all but the last few files of a big batch.
                if library
                    .update(cx, |library, _| {
                        library.note_self_write([edit.path.clone()])
                    })
                    .is_err()
                {
                    return;
                }
                let (edit, result) = cx
                    .background_executor()
                    .spawn(async move {
                        // Through the key: a cue track's edit stays in the
                        // library, since its image belongs to the whole disc.
                        let r = writer::commit_key(&edit.path, sub, &edit.changes, &edit.pictures);
                        (edit, r)
                    })
                    .await;
                match result {
                    Ok(()) => {
                        committed.push(edit);
                        committed_subs.push(sub);
                    }
                    Err(e) => {
                        failures += 1;
                        if first_error.is_none() {
                            let name = edit
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| edit.path.display().to_string());
                            first_error = Some(format!("{name}: {e}"));
                        }
                    }
                }
                // A closed window (the user cancelled) drops the handle;
                // stop rather than keep writing into nothing.
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
            this.update_in(cx, move |this, window, cx| {
                // A written file's baseline follows the write, so a retry
                // after a partial failure diffs against what is on disk
                // now instead of re-committing the landed files.
                for edit in &committed {
                    let Some(ix) = this.tracks.iter().position(|t| t.path == edit.path) else {
                        continue;
                    };
                    let Some(baseline) = this.baselines.as_mut().and_then(|b| b.get_mut(ix)) else {
                        continue;
                    };
                    for change in &edit.changes {
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
                            format!("{failures} files failed; {e}").into()
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

    /// The selection as a list: the display line filling left, the
    /// duration right, one hairline row per track. The line rides a bare
    /// disabled input rather than plain text so it can be selected and
    /// copied; the component only gates typing on disabled, never
    /// selection or copy.
    fn track_section(&self) -> Div {
        let mut body = div().flex().flex_col();
        for track in &self.tracks {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .py(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&track.line)
                                .small()
                                .appearance(false)
                                .disabled(true),
                        ),
                    )
                    .when(track.duration_ms > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(palette::text_muted())
                                .child(fmt_ms(track.duration_ms)),
                        )
                    }),
            );
        }
        section("Tracks", None, body)
    }

    /// The tags no field addresses, read-only under their own fold: TXXX
    /// descriptions, the keys lofty maps that the form has no row for,
    /// and the binary frames named by size. Every save carries them
    /// through untouched, so the point here is only to show that they
    /// exist. The header is hand-rolled rather than [`section`]'s
    /// because the count moves with the selection and that one takes a
    /// static label.
    fn unknown_section(&self, cx: &mut Context<Self>) -> Option<Div> {
        let unknowns = self.unknowns.as_ref()?;
        if unknowns.rows.is_empty() && unknowns.failed == 0 {
            return None;
        }
        let open = self.unknowns_open;
        let mut body = div().flex().flex_col();
        if unknowns.failed > 0 {
            body = body.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(format!(
                        "{} of {} files' tags couldn't be read",
                        unknowns.failed, unknowns.files
                    )),
            );
        }
        for row in &unknowns.rows {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .py(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .w(px(180.))
                            .flex_none()
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(row.key.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(row.value.clone()))
                    // A key only some of the selection carries says so;
                    // one they all carry needs no note.
                    .when(row.files < unknowns.files, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(format!("{} of {}", row.files, unknowns.files)),
                        )
                    }),
            );
        }
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
                        .gap(tokens::SPACE_XS)
                        .pb(tokens::SPACE_XS)
                        .border_b_1()
                        .border_color(palette::border())
                        .text_xs()
                        .text_color(palette::text_muted())
                        .cursor_pointer()
                        .hover(|d| d.text_color(palette::text()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.toggle_unknowns(cx)),
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
                        .child(format!("Other Tags ({})", unknowns.rows.len())),
                )
                .when(open, |d| d.child(body)),
        )
    }

    /// Table mode's one line about the read-only tag list, which only the
    /// form shows: the grid is one field per column and these keys are
    /// ragged per file, so there is no honest column for them. The count
    /// sits under the table and the click hands the user to the section
    /// that can show them.
    fn unknown_hint(&self, cx: &mut Context<Self>) -> Option<Div> {
        let count = self
            .unknowns
            .as_ref()
            .map(|unknowns| unknowns.rows.len())
            .filter(|count| *count > 0)?;
        Some(
            div()
                .flex_none()
                .mt(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_muted())
                .cursor_pointer()
                .hover(|d| d.text_color(palette::text()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.show_unknowns(window, cx)),
                )
                .child(format!("Other Tags ({count}) in form view")),
        )
    }

    /// Leave the table for the form with the read-only tag list open, the
    /// hint's landing.
    fn show_unknowns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table {
            self.toggle_table(window, cx);
        }
        self.unknowns_open = true;
        cx.notify();
    }

    /// The tags section: the shared form, or in table mode the per-track
    /// grid. The lookup rides beside the heading's name, the mode toggle,
    /// the column picker, and the guess panel its right edge, since each
    /// is about what the section shows; save and cancel belong to the
    /// window and ride its footer.
    fn tags_section(&self, cx: &mut Context<Self>) -> Div {
        // The online lookup is the form's alone, single-track only: the
        // compare matches on one track's tags, so a batch has no one
        // query, and in the table every row carries its own. Gated on
        // the provider toggle like the metadata panel's.
        let single = self.tracks.len() == 1;
        let look_up = (!self.table && single && providers::metadata_online()).then(|| {
            settings_ui::small_button(
                "Look Up",
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
            .when(self.table, |d| d.child(self.columns_menu(cx)))
            .child(settings_ui::small_button(
                if self.table { "Form" } else { "Table" },
                icons::ROWS_3,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.toggle_table(window, cx)),
            ))
            .child(settings_ui::small_button(
                "Guess",
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
        // so nothing edits out from under the write. Cancel sits outside
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
            Some(control) => section_with_control("Tags", control, Some(buttons), content),
            None => section("Tags", Some(buttons), content),
        }
    }

    /// The table's column picker: one checked row per column, ticked
    /// while shown, reading off the editor's own set so the menu never
    /// touches the table mid-update.
    fn columns_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let hidden = self.hidden.clone();
        Button::new("tag-columns")
            .label("Columns")
            .small()
            .outline()
            .dropdown_menu(move |mut menu, _, _| {
                for key in column_keys() {
                    let this = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(title_case(key))
                            .checked(!hidden.contains(key))
                            .on_click(move |_, _, cx| {
                                if let Some(this) = this.upgrade() {
                                    this.update(cx, |this, cx| this.toggle_column(key, cx));
                                }
                            }),
                    );
                }
                menu
            })
    }

    /// Whether a save can run as it stands. Baselines are what a commit
    /// diffs each file against, so there is nothing safe to write until
    /// they land, and a commit already in flight owns the files.
    fn savable(&self) -> bool {
        !self.saving && self.baselines.is_some()
    }

    /// The window's own actions: the save, the way out, and what's
    /// holding the save back when something is. It hangs off the root
    /// rather than either page, so the buttons keep their place when the
    /// form and the table swap.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let hint: gpui::AnyElement = if self.saving {
            // A commit runs off the UI thread, so say it plainly. The
            // count names how far a slow batch has got instead of
            // freezing on a mute spinner.
            let label = if self.save_total > 1 {
                let at = (self.save_done + 1).min(self.save_total);
                format!("Saving {}/{}...", at, self.save_total)
            } else {
                "Saving...".to_string()
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
            // A format the writer has no path for is not a broken file,
            // so it gets its own line rather than wearing the parse error
            // of the read that never happened.
            let reason: Option<SharedString> = if self.unsupported > 0 {
                Some(if self.unsupported == self.tracks.len() {
                    "Tags for this format can't be read or written yet.".into()
                } else {
                    "Some of these files are in a format whose tags can't be read or written yet."
                        .into()
                })
            } else if self.error.is_some() {
                self.error.clone()
            } else if self.baselines.is_none() {
                Some("Loading tags...".into())
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
                        "Cancel",
                        icons::CLOSE,
                        false,
                        cx.listener(|this, _, window, cx| {
                            this.persist_frame(window, cx);
                            window.remove_window();
                        }),
                    )),
            )
    }

    /// The shared form: one bare field per row - no input chrome, the
    /// sheet look. Per-track fields have no single form value in a
    /// batch, so they read as plain text and the table edits them.
    fn form_body(&self, cx: &mut Context<Self>) -> Div {
        let single = self.tracks.len() == 1;
        let rows = FIELDS
            .iter()
            .enumerate()
            .map(|(i, (field_def, label, per_track))| {
                // A mixed batch field can be wiped across every file: its
                // box is empty over the placeholder, so typing can only add
                // a value, never say "clear it everywhere". The toggle does.
                let clearable =
                    !single && !per_track && self.mixed.get(i).copied().unwrap_or(false);
                let cleared = self.cleared.get(i).copied().unwrap_or(false);
                let field: gpui::AnyElement = if *per_track && !single {
                    let value = self.inputs[i].read(cx).value();
                    let (text, faded) = if self.mixed.get(i).copied().unwrap_or(false) {
                        (SharedString::from("Multiple values"), true)
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
                    // the way; the walk itself is the stock next stop,
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
                            .w(px(84.))
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(*label),
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
                                .child(if cleared { "will clear" } else { "clear all" })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toggle_clear(i, window, cx)
                                })),
                        )
                    })
            });
        div().flex().flex_col().gap(px(2.)).children(rows)
    }

    /// The table over the grid: resizable, sortable columns like the
    /// library's list, every field editable per track. Tab walks each
    /// column top to bottom.
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
/// the sort permutation from display row to track index. The editor comes
/// along weak so a row's own lookup can reach it from the cell.
struct CellGrid {
    columns: Vec<Column>,
    cells: Vec<Vec<Entity<InputState>>>,
    names: Vec<Entity<InputState>>,
    order: Vec<usize>,
    editor: WeakEntity<TagEditor>,
}

/// The file column, then one per field: name columns wide, numeric ones
/// narrow, all resizable and sortable like the library's list. `saved`
/// overrides the defaults with the last editor's widths, one slot per
/// column in the full order. Those widths are positional, so a set
/// written before a column existed falls back to the defaults rather
/// than landing on the wrong columns. `hidden` columns drop out after
/// the widths resolve; a hidden set that would empty the table is
/// ignored, since an empty table has no header to bring one back from.
fn grid_columns(saved: &[f32], hidden: &HashSet<String>) -> Vec<Column> {
    let defaults = default_widths();
    let saved = if saved.len() == defaults.len() {
        saved
    } else {
        &[]
    };
    let width = |i: usize| {
        saved
            .get(i)
            .copied()
            .filter(|w| *w >= 24.)
            .unwrap_or(defaults[i])
    };
    let columns: Vec<Column> = column_keys()
        .enumerate()
        .map(|(i, key)| {
            Column::new(key, title_case(key))
                .width(px(width(i)))
                .sortable()
        })
        .collect();
    let shown: Vec<Column> = columns
        .iter()
        .filter(|column| !hidden.contains(column.key.as_ref()))
        .cloned()
        .collect();
    if shown.is_empty() {
        columns
    } else {
        shown
    }
}

impl CellGrid {
    /// Which [`FIELDS`] entry a column edits, or None for a display
    /// column like the file name. By key rather than position: hidden
    /// columns leave the display order sparse.
    fn field_ix(&self, col_ix: usize) -> Option<usize> {
        let key = self.columns[col_ix].key.clone();
        FIELDS
            .iter()
            .position(|(_, label, _)| *label == key.as_ref())
    }

    /// Whether each [`FIELDS`] entry has a column on screen, for the tab
    /// walk: a hidden column's cells exist and keep their edits, but
    /// focusing one would land the cursor somewhere the table doesn't
    /// draw.
    fn visible_fields(&self) -> Vec<bool> {
        let mut visible = vec![false; FIELDS.len()];
        for ix in 0..self.columns.len() {
            if let Some(field) = self.field_ix(ix) {
                visible[field] = true;
            }
        }
        visible
    }

    /// The file column's cell: the name on a bare disabled input so its
    /// text selects and copies the way the track list's lines do (the
    /// component only gates typing on disabled, never selection), and
    /// the row's own lookup beside it. A lookup matches one file's tags
    /// against a release, so once the grid is showing many files the row
    /// is the only honest place for it.
    fn file_cell(&self, track: usize) -> Div {
        let editor = self.editor.clone();
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
            // Gated on the provider toggle like the header's, which the
            // form still carries for a single track.
            .when(providers::metadata_online(), |d| {
                d.child(settings_ui::icon_button(
                    icons::DOWNLOAD,
                    false,
                    move |_, window, cx| {
                        editor
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
        let field = self.field_ix(col_ix);
        let numeric = field.is_some_and(|i| {
            matches!(
                FIELDS[i].0,
                Field::Year | Field::TrackNo | Field::DiscNo | Field::Rating
            )
        });
        let mut keyed: Vec<(usize, String)> = self
            .order
            .iter()
            .map(|&t| {
                let value = match field {
                    Some(i) => self.cells[t][i].read(cx).value().to_lowercase(),
                    None => self.names[t].read(cx).value().to_lowercase(),
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
        // A display column edits nothing and stays out of the tab walk.
        let Some(col_ix) = self.field_ix(col_ix) else {
            return self.file_cell(track).into_any_element();
        };
        let total = rows * FIELDS.len();
        let cell = self.cells[track][col_ix].clone();
        // Star-style rating cells hold no focusable input: they render
        // the click control and sit outside the tab walk. The numeric
        // style keeps them as plain 0-10 inputs in the walk below.
        let stars = rating_style() == RatingStyle::Stars;
        if stars && FIELDS[col_ix].0 == Field::Rating {
            return div()
                .h_full()
                .flex()
                .items_center()
                .child(rating_field(&cell, cx))
                .into_any_element();
        }
        // The neighbors down and up the column, wrapping into the next
        // and previous column at the ends and skipping unfocusable
        // rating columns and fields whose column is toggled away.
        let visible = self.visible_fields();
        let at = |pos: usize| {
            let (col, row) = (pos / rows, pos % rows);
            self.cells[self.order[row]][col].read(cx).focus_handle(cx)
        };
        let step = move |from: usize, dir: i64| {
            let mut pos = from;
            loop {
                pos = (pos as i64 + dir).rem_euclid(total as i64) as usize;
                let field = pos / rows;
                if visible[field] && !(stars && FIELDS[field].0 == Field::Rating) {
                    return pos;
                }
            }
        };
        let pos = col_ix * rows + row_ix;
        let next = at(step(pos, 1));
        let prev = at(step(pos, -1));
        // Tab walks the column, not the row: the editor's own binding
        // catches it here, deeper than the window root's, and moves to
        // the neighbor we compute instead of the paint-order stop.
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

/// The selection's unknown tags as one list: every key any file carries,
/// ordered by how many carry it so the shared ones lead, alphabetical
/// inside a tie so the order holds still across opens. Files that agree
/// on a value show it; the rest say so the way the form's mixed fields
/// do.
fn gather_unknowns(reads: &[FileRead]) -> UnknownTags {
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
            let value = if agreed {
                one_line(&values[0].display())
            } else {
                "Multiple values".to_owned()
            };
            UnknownRow {
                key: one_line(&key).into(),
                value: value.into(),
                files: files.len(),
            }
        })
        .collect();
    UnknownTags {
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
        // The table scrolls its own rows inside a fixed page - its rows
        // are the tracks, so the track list would only repeat them. The
        // form page scrolls whole under the shared scrollbar.
        let page: gpui::AnyElement = if self.table {
            div()
                .size_full()
                .flex()
                .flex_col()
                .p(tokens::SPACE_MD)
                .child(self.tags_section(cx).flex_1().min_h_0())
                .children(self.unknown_hint(cx))
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
                        .child(self.track_section())
                        .children(self.unknown_section(cx)),
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
                        // the window's, the same as the settings page. Two
                        // layers is what the backdrop reads through, and the
                        // footer stays outside it to sit a step darker.
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
