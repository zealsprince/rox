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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, FocusHandle,
    Focusable as _, Global, KeyBinding, ScrollHandle, SharedString, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::spinner::Spinner;
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Root, Sizable, Size};

use rox_library::projection::Projection;
use rox_library::rating;
use rox_library::writer::{self, Change, Edit, Field};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::panels::library::{fmt_ms, Library};
use crate::providers;
use crate::settings::{rating_style, RatingStyle, Settings};
use crate::settings_ui::{self, section, SECTION_GAP};
use crate::tags::suggest;

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
    crate::rating_ui::control(current, move |value, window, cx| {
        let text = if value == 0 {
            String::new()
        } else {
            rating::display(value)
        };
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    })
}

actions!(tag_editor, [FieldTab, FieldTabPrev]);

/// The editor's tab bindings; call once at startup. They scope to the
/// field wrappers' key context, deeper along the focus path than the
/// window root's own tab bindings, so inside a tag field the editor owns
/// what tab means: take the open suggestion, then move. Bindings win
/// over key listeners, so a listener could never have seen the key.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FieldTab, Some("TagField")),
        KeyBinding::new("shift-tab", FieldTabPrev, Some("TagField")),
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

/// Open a tag editor on `ids`, the selection's tracks in view order, or
/// bring the editor already on that selection to the front. An empty
/// selection opens nothing.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    let entries = cx
        .try_global::<OpenTagEditors>()
        .map(|open| open.0.clone())
        .unwrap_or_default();
    // Closed windows fall out of the list as a side effect of the probe.
    let mut alive = Vec::with_capacity(entries.len() + 1);
    let mut focused = false;
    for (entry_key, handle) in entries {
        let matches = entry_key == key;
        if handle
            .update(cx, |_, window, _| {
                if matches {
                    window.activate_window();
                }
            })
            .is_ok()
        {
            focused |= matches;
            alive.push((entry_key, handle));
        }
    }
    if focused {
        cx.set_global(OpenTagEditors(alive));
        return;
    }
    // The last closed editor's size, sanity-floored; the default is wide
    // enough that the table's columns fit without scrolling.
    let (width, height) = Settings::load()
        .tag_editor
        .filter(|s| s.width >= 400. && s.height >= 300.)
        .map(|s| (s.width, s.height))
        .unwrap_or((1400., 680.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - Tag Editor".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - Tag Editor");
            let view = cx.new(|cx| TagEditor::new(state, ids, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the tag editor window");
    alive.push((key, handle));
    cx.set_global(OpenTagEditors(alive));
}

/// One selected track as the list shows it, resolved at open; the path is
/// what the baselines read and the commits write.
struct TrackRow {
    path: PathBuf,
    title: SharedString,
    /// The row's display line (title, artist when tagged) in a read-only
    /// input, so its text selects and copies into the fields - the way
    /// into retagging files whose only metadata is their name.
    line: Entity<InputState>,
    duration_ms: u32,
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
    /// What each cell last seeded from. A cell still on its seed follows
    /// re-seeds (a form edit folding in); one the user moved is theirs.
    seeds: Vec<Vec<SharedString>>,
    /// The projection the suggestion providers share, kept for cells
    /// created after open.
    projection: Option<Arc<Projection>>,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// A commit is in flight; the fields lock and the buttons hold still
    /// until it lands.
    saving: bool,
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
                    Some((v.title.to_owned(), v.artist.to_owned(), v.duration_ms))
                });
                let (title, artist, duration_ms) = resolved.unwrap_or_else(|| {
                    let title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    (title, String::new(), 0)
                });
                tracks.push((path, title, artist, duration_ms));
            }
            tracks
        };
        let tracks: Vec<TrackRow> = tracks
            .into_iter()
            .map(|(path, title, artist, duration_ms)| {
                let mut line = title.clone();
                if !artist.is_empty() {
                    line.push_str(" - ");
                    line.push_str(&artist);
                }
                let line = cx.new(|cx| InputState::new(window, cx).default_value(line));
                TrackRow {
                    path,
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
        let _input_events = inputs
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
            seeds: Vec::new(),
            projection,
            error: None,
            saving: false,
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
    /// its baseline there is nothing safe to diff that file against.
    fn read_baselines(&self, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.tracks.iter().map(|track| track.path.clone()).collect();
        cx.spawn_in(window, async move |this, cx| {
            let reads = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .map(|path| writer::read(path))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let mut baselines = Vec::with_capacity(reads.len());
                for (read, track) in reads.into_iter().zip(&this.tracks) {
                    match read {
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
                window.focus(&cells[0][0].read(cx).focus_handle(cx));
            }
        }
        cx.notify();
    }

    /// Write the window frame and column widths into the settings file,
    /// the restore for the next editor. Runs on every close path; with
    /// several editors open the last writer wins.
    fn persist_frame(&self, window: &Window, cx: &App) {
        let frame = window.window_bounds().get_bounds();
        let columns: Vec<f32> = self
            .grid
            .as_ref()
            .map(|grid| {
                grid.read(cx)
                    .delegate()
                    .columns
                    .iter()
                    .map(|column| column.width.into())
                    .collect()
            })
            .unwrap_or_default();
        Settings::update(move |s| {
            let state = s.tag_editor.get_or_insert_with(Default::default);
            state.width = frame.size.width.into();
            state.height = frame.size.height.into();
            // A form-only session has no table; keep the saved widths.
            if !columns.is_empty() {
                state.columns = columns;
            }
        });
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
                window.focus(&cells[0][0].read(cx).focus_handle(cx));
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
        if self.cells.is_none() {
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
            let saved = Settings::load()
                .tag_editor
                .map(|s| s.columns)
                .unwrap_or_default();
            let delegate = CellGrid {
                columns: grid_columns(&saved),
                cells: cells.clone(),
                order: (0..cells.len()).collect(),
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
                input.set_placeholder(if mixed { "Multiple values" } else { "" }, window, cx);
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

    /// Open the metadata compare on the single edited track. The window
    /// searches, ranks matches, and on apply calls back into
    /// [`Self::fill_fields`] rather than writing, so this editor stays the
    /// one writer. Single-track only, the button its gate.
    fn look_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(track) = self.tracks.first() else {
            return;
        };
        let path = track.path.clone();
        let library = self.library.clone();
        let now_art = self.now_art.clone();
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        crate::tags::matcher::open_fill(library, now_art, path, weak, handle, cx);
    }

    /// Fill the form from a looked-up match, one field at a time: each set
    /// input drifts from its fill and arms as a pending edit, so the
    /// normal save writes it and nothing lands until the user saves.
    /// Fields the match does not carry are left untouched. The compare
    /// calls this on its own apply, on this editor's window.
    pub fn fill_fields(
        &mut self,
        values: &[(Field, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (field, value) in values {
            let Some(i) = FIELDS.iter().position(|(f, _, _)| f == field) else {
                continue;
            };
            let value = value.clone();
            self.inputs[i].update(cx, |input, cx| input.set_value(value, window, cx));
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
        let (Some(baselines), false) = (&self.baselines, self.saving) else {
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
                edits.push(Edit {
                    path: track.path.clone(),
                    changes,
                    pictures: Vec::new(),
                });
            }
        }
        if edits.is_empty() {
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
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for edit in edits {
                // Note the write before it lands so the watch batch it
                // triggers is suppressed, not reindexed. The apply_edits at
                // the end notes too, but by then the suppression window has
                // long passed for all but the last few files of a big batch.
                if library
                    .update(cx, |library, _| library.note_self_write([edit.path.clone()]))
                    .is_err()
                {
                    return;
                }
                let (edit, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let r = writer::commit_with(&edit.path, &edit.changes, &edit.pictures);
                        (edit, r)
                    })
                    .await;
                match result {
                    Ok(()) => committed.push(edit),
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
                    library.update(cx, |library, cx| library.apply_edits(&committed, cx));
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

    /// The tags section: the shared form, or in table mode the per-track
    /// grid; the mode toggle, save, and cancel ride the section header,
    /// the error inline under the fields per the metadata panel's edit
    /// face.
    fn tags_section(&self, cx: &mut Context<Self>) -> Div {
        // The online lookup rides the header, single-track only: the
        // compare matches on one track's tags, so a batch has no query.
        // Gated on the provider toggle like the metadata panel's.
        let single = self.tracks.len() == 1;
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            // A commit runs off the UI thread, so say it plainly: the
            // spinner and a running count ride ahead of the buttons until
            // the write lands or fails. The count names how far a slow
            // batch has got instead of freezing on a mute spinner.
            .when(self.saving, |d| {
                let label = if self.save_total > 1 {
                    let at = (self.save_done + 1).min(self.save_total);
                    format!("Saving {}/{}...", at, self.save_total)
                } else {
                    "Saving...".to_string()
                };
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(Spinner::new().with_size(Size::Small))
                        .child(label),
                )
            })
            .when(single && providers::metadata_online(), |d| {
                d.child(settings_ui::small_button(
                    "Look Up",
                    icons::DOWNLOAD,
                    self.saving || self.baselines.is_none(),
                    cx.listener(|this, _, window, cx| this.look_up(window, cx)),
                ))
            })
            .child(settings_ui::small_button(
                if self.table { "Form" } else { "Table" },
                icons::ROWS_3,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.toggle_table(window, cx)),
            ))
            .child(settings_ui::small_button(
                "Save",
                icons::CHECK,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.save(window, cx)),
            ))
            // Cancel stays live through a save: a slow or wedged commit
            // needs a way out, and the atomic writer leaves every original
            // intact whether the batch finished or not.
            .child(settings_ui::small_button(
                "Cancel",
                icons::CLOSE,
                false,
                cx.listener(|this, _, window, cx| {
                    this.persist_frame(window, cx);
                    window.remove_window();
                }),
            ))
            .into_any_element();
        let body = if self.table {
            self.table_body()
        } else {
            self.form_body(cx).into_any_element()
        };
        section(
            "Tags",
            Some(buttons),
            div()
                .flex()
                .flex_col()
                .when(self.table, |d| d.flex_1().min_h_0())
                .child(
                    // The fields and the grid lock while a commit is in
                    // flight: a transparent occluder over them swallows
                    // clicks and keystrokes so nothing edits out from under
                    // the write. Cancel sits above it, on the header.
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .when(self.table, |d| d.flex_1().min_h_0())
                        .child(body)
                        .when(self.saving, |d| {
                            d.child(div().absolute().inset_0().occlude())
                        }),
                )
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .mt(tokens::SPACE_XS)
                            .text_color(palette::text_muted())
                            .child(error),
                    )
                }),
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
                let clearable = !single && !per_track && self.mixed.get(i).copied().unwrap_or(false);
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
/// entity, so the table shows exactly the state save reads. `order` is
/// the sort permutation from display row to track index.
struct CellGrid {
    columns: Vec<Column>,
    cells: Vec<Vec<Entity<InputState>>>,
    order: Vec<usize>,
}

/// One column per field: name columns wide, numeric ones narrow, all
/// resizable and sortable like the library's list. `saved` overrides the
/// defaults with the last editor's widths, in field order.
fn grid_columns(saved: &[f32]) -> Vec<Column> {
    FIELDS
        .iter()
        .enumerate()
        .map(|(i, (field, label, _))| {
            let default = match field {
                Field::Year | Field::TrackNo | Field::DiscNo => 64.,
                // Room for five stars or the numeric strip.
                Field::Rating => 96.,
                _ => 150.,
            };
            let width = saved
                .get(i)
                .copied()
                .filter(|w| *w >= 24.)
                .unwrap_or(default);
            Column::new(*label, *label).width(px(width)).sortable()
        })
        .collect()
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
        let numeric = matches!(
            FIELDS[col_ix].0,
            Field::Year | Field::TrackNo | Field::DiscNo | Field::Rating
        );
        let mut keyed: Vec<(usize, String)> = self
            .order
            .iter()
            .map(|&t| (t, self.cells[t][col_ix].read(cx).value().to_lowercase()))
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
        let total = rows * self.columns.len();
        let cell = self.cells[self.order[row_ix]][col_ix].clone();
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
        // rating columns.
        let at = |pos: usize| {
            let (col, row) = (pos / rows, pos % rows);
            self.cells[self.order[row]][col].read(cx).focus_handle(cx)
        };
        let step = |from: usize, dir: i64| {
            let mut pos = from;
            loop {
                pos = (pos as i64 + dir).rem_euclid(total as i64) as usize;
                if !(stars && FIELDS[pos / rows].0 == Field::Rating) {
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
                        .child(self.track_section()),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the page; without it
            // translucent surfaces would sink into the window's own
            // black instead of the playing track's art.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .bg(palette::bg_elevated())
                    .child(page)
                    // Always visible, not fading in on scroll: the thumb
                    // is what says more page hangs below the fold.
                    .when(!self.table, |d| {
                        d.child(div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                        ))
                    }),
            )
    }
}
