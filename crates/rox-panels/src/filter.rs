//! The filter panel: the library's field values as cascading columns,
//! each one filter field (artist, album artist, album, genre, or year)
//! listing every distinct value with its track count. Picking values
//! writes the shared query's structured filter, so every global-following
//! panel narrows with it, and each column here narrows by the picks in
//! the columns left of it, the column-browser cascade. The shared text
//! query narrows the value lists too, so the panel and the search boxes
//! read the same library. Columns are per-panel config; the picks are the
//! one app-wide filter, so two filter panels share them.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::projection::{FilterField, FilterSet, Projection, SymTable};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::query::shared_query::SharedQueryEvent;

/// One value row's height; the lists are uniform_lists, so every row is
/// the same.
const ROW_H: f32 = 26.;

/// The most values one column lists. A library big enough to pass this
/// has more distinct albums than anyone scrolls through, and the values
/// past the cap cost real memory (a `SharedString` and a `String` each)
/// for a list nobody reads to the end of. Over the cap the column keeps
/// the most-used values and says how many it left out, which is a
/// narrower answer than the truth but a usable one; typing in the search
/// box narrows the context until everything fits again.
const VALUE_CAP: usize = 5000;

/// How many rows one core counts at a time. The work per row is an array
/// index and an increment, so the chunk has to be big or the split costs
/// more than the pass; each chunk also holds its own counter table, which
/// is what keeps this from being a chunk per thousand rows.
const COUNT_CHUNK: usize = 256 * 1024;

/// How long a keystroke-driven rebuild waits for the next keystroke. The
/// picks don't wait: a click has nothing more coming behind it, and the
/// row it lit up should fill in on the same frame it was clicked.
const REBUILD_DEBOUNCE: Duration = Duration::from_millis(100);

/// How long a type-ahead phrase keeps growing before the next keystroke
/// starts a fresh jump.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// A column's filter field, the per-panel half of the story; the picks
/// themselves are stored on the shared query.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
}

impl ColumnKind {
    /// Every pickable kind, in the menus' order.
    pub const ALL: [ColumnKind; 5] = [
        ColumnKind::Artist,
        ColumnKind::AlbumArtist,
        ColumnKind::Album,
        ColumnKind::Genre,
        ColumnKind::Year,
    ];

    fn label(self) -> &'static str {
        match self {
            ColumnKind::Artist => rox_i18n::t_static("filter-field-artist"),
            ColumnKind::AlbumArtist => rox_i18n::t_static("filter-field-album-artist"),
            ColumnKind::Album => rox_i18n::t_static("filter-field-album"),
            ColumnKind::Genre => rox_i18n::t_static("filter-field-genre"),
            ColumnKind::Year => rox_i18n::t_static("filter-field-year"),
        }
    }

    fn field(self) -> FilterField {
        match self {
            ColumnKind::Artist => FilterField::Artist,
            ColumnKind::AlbumArtist => FilterField::AlbumArtist,
            ColumnKind::Album => FilterField::Album,
            ColumnKind::Genre => FilterField::Genre,
            ColumnKind::Year => FilterField::Year,
        }
    }
}

/// The filter panel's per-view config: what a saved layout restores. The
/// columns only; the picks are shared app state, transient like the
/// query text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The column layout, one filter field per column, left to right.
    pub columns: Vec<ColumnKind>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        FilterConfig {
            chrome: PanelChrome::default(),
            columns: vec![ColumnKind::Artist, ColumnKind::Album],
        }
    }
}

/// One value row: the display label, the exact value the filter matches,
/// how many context tracks have it, and whether it's picked.
struct Value {
    label: SharedString,
    value: String,
    count: u32,
    selected: bool,
}

/// A header drag in flight: the column it started from, so a drop on
/// another header can tell what to move. The label comes along for the
/// preview.
#[derive(Clone)]
struct ColumnDrag {
    from: usize,
    label: SharedString,
}

/// The chip that floats under the pointer while a column is dragged.
struct ColumnDragPreview {
    label: SharedString,
}

impl Render for ColumnDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .text_color(palette::text())
            .child(self.label.clone())
    }
}

pub struct FilterPanel {
    state: AppState,
    config: FilterConfig,
    /// Per column: its value rows, rebuilt when the library, the shared
    /// query, or the picks change, never per frame.
    columns: Vec<Vec<Value>>,
    /// Per column: how many values [`VALUE_CAP`] left out, zero while the
    /// column lists everything it found. What the notice row under the
    /// list counts.
    over_cap: Vec<usize>,
    /// Per column: whether its values are a rebuild behind, because the
    /// column was just added or its field just changed. The lists move
    /// with the config the moment it changes, so a header never sits over
    /// another field's values; this is what tells a click that the empty
    /// list under it is waiting rather than genuinely empty.
    pending: Vec<bool>,
    /// Bumped per scheduled rebuild; a pass whose number has moved on by
    /// the time it lands is a pass whose answer is already stale.
    rebuild_gen: u64,
    /// The query text the current lists were built from, so an arriving
    /// change can tell typing (which waits for the pause) from a pick
    /// (which doesn't).
    applied_text: String,
    scrolls: Vec<UniformListScrollHandle>,
    /// The column the keyboard drives: type-ahead and arrows move within
    /// it, and the cursor highlight is in it. Set by clicking a value or
    /// stepping left and right.
    active_col: usize,
    /// The keyboard cursor, a row index in the active column: where arrows
    /// move from and enter toggles. None until a key or click sets one.
    cursor: Option<usize>,
    /// The type-ahead phrase and when its last keystroke arrived, so a quick
    /// run of letters jumps to a value by prefix.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
    /// Drops the phrase when focus leaves the panel, so tab goes back to
    /// walking panels instead of cycling a phrase from a past visit.
    _type_ahead_blur: Subscription,
}

impl FilterPanel {
    pub fn new(
        state: AppState,
        config: FilterConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        // The picks arrive here too: a toggle writes the shared filter, the
        // Changed comes back around, and the cascade rebuilds once. Only
        // the text half is typed, so only it waits out the debounce.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, query, _: &SharedQueryEvent, cx| {
                let typed = query.read(cx).text() != this.applied_text;
                this.schedule_refresh(typed, cx);
            },
        );
        let focus = cx.focus_handle().tab_stop(true);
        // The phrase outlives its badge, so it needs an end: leaving the
        // panel drops it, which is also what hands tab back to traversal.
        let panel = cx.weak_entity();
        let _type_ahead_blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
            panel
                .update(cx, |this: &mut FilterPanel, cx| {
                    this.clear_type_ahead(cx);
                })
                .ok();
        });
        let mut this = FilterPanel {
            state,
            config,
            columns: Vec::new(),
            over_cap: Vec::new(),
            pending: Vec::new(),
            rebuild_gen: 0,
            applied_text: String::new(),
            scrolls: Vec::new(),
            active_col: 0,
            cursor: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            focus,
            tab_panel: None,
            _library_changed,
            _query_changed,
            _type_ahead_blur,
        };
        this.refresh(cx);
        this
    }

    /// Browse from the keyboard while the panel is focused: up and down move
    /// the active column's cursor, enter toggles the cursor's value, and
    /// plain typing jumps to the next value by prefix. The active column is
    /// the last one clicked. Left and right stay the workspace's seek, and
    /// space its play/pause unless a phrase is mid-flight.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        match keystroke.key.as_str() {
            // Escape drops a phrase, which is what hands tab back to
            // panel traversal.
            "escape" => {
                self.clear_type_ahead(cx);
            }
            "up" => self.move_cursor(-1, cx),
            "down" => self.move_cursor(1, cx),
            "home" => self.set_cursor(0, cx),
            "end" => {
                let last = self.active_len().saturating_sub(1);
                self.set_cursor(last, cx);
            }
            "enter" => {
                if let Some(ix) = self.cursor {
                    if let Some(value) = self.value_at(self.active_col, ix) {
                        self.toggle(self.active_col, value, cx);
                    }
                }
            }
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                // Space plays/pauses unless a phrase is actually mid-flight. The
                // phrase never clears on its own, so an emptiness test alone
                // would treat a phrase typed minutes ago as live and keep
                // swallowing space. Gate on the type-ahead window instead.
                let phrase_live =
                    !self.type_ahead.is_empty() && panel::type_ahead_live(self.type_ahead_at);
                if !phrase_live && text == " " {
                    return;
                }
                // Consumed as type-ahead text: stop it here so it doesn't
                // also match the workspace's space-bound TogglePlayback
                // binding, which this panel otherwise inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead phrase and jump to its next match in
    /// the active column. A grown phrase re-tests the cursor's own row first
    /// so refining a match stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let now = Instant::now();
        let grown = self
            .type_ahead_at
            .is_some_and(|at| now.duration_since(at) < TYPE_AHEAD);
        if grown {
            self.type_ahead.push_str(&text);
        } else {
            self.type_ahead = text;
        }
        self.type_ahead_at = Some(now);
        // The badge shows the phrase now and leaves when the window
        // lapses; a miss below still updated it, so repaint either way.
        panel::type_ahead_fade(cx);
        cx.notify();
        let Some(values) = self.columns.get(self.active_col) else {
            return;
        };
        let needle = self.type_ahead.to_lowercase();
        // A grown phrase re-tests the current row; a fresh one starts past
        // it, so the same first letter steps to the next match.
        let start = match self.cursor {
            Some(ix) if grown => ix,
            Some(ix) => ix + 1,
            None => 0,
        };
        let len = values.len();
        let hit = (0..len)
            .map(|off| (start + off) % len)
            .find(|&ix| panel::type_ahead_hit(&values[ix].label.to_lowercase(), &needle));
        if let Some(ix) = hit {
            self.set_cursor(ix, cx);
        }
    }

    /// Drop the phrase, handing tab back to Root's panel traversal. True
    /// when there was one, for the escape ladder.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Step to the phrase's neighbouring match, Tab's cycle, dispatched
    /// off the cycle-scoped tab bindings. Deliberately leaves the window
    /// stamp alone: the badge and the letter grouping belong to typing,
    /// so a run of tabs steps silently rather than reviving them.
    fn type_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.type_ahead.is_empty() {
            return;
        }
        if self.active_len() == 0 {
            return;
        }
        cx.notify();
        let needle = self.type_ahead.to_lowercase();
        let hit = {
            let values = &self.columns[self.active_col];
            panel::type_ahead_scan(values.len(), self.cursor, back)
                .find(|&ix| panel::type_ahead_hit(&values[ix].label.to_lowercase(), &needle))
        };
        if let Some(ix) = hit {
            self.set_cursor(ix, cx);
        }
    }

    /// The active column's value count.
    fn active_len(&self) -> usize {
        self.columns.get(self.active_col).map(Vec::len).unwrap_or(0)
    }

    /// One column's value string at a row, for toggling from the keyboard.
    fn value_at(&self, col: usize, ix: usize) -> Option<String> {
        self.columns.get(col)?.get(ix).map(|v| v.value.clone())
    }

    /// Step the cursor within the active column; the first press with no
    /// cursor starts at the edge it heads toward.
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let ix = match self.cursor {
            None if delta >= 0 => 0,
            None => len - 1,
            Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.set_cursor(ix, cx);
    }

    /// Put the cursor on a row of the active column and scroll it into view.
    fn set_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.active_len() {
            return;
        }
        self.cursor = Some(ix);
        if let Some(scroll) = self.scrolls.get(self.active_col) {
            scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Rebuild every column's values right away, for the changes that
    /// aren't typed: a rescan, a column added or dropped, a pick. The
    /// cascade itself is [`build_columns`].
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.schedule_refresh(false, cx);
    }

    /// Schedule the cascade. The counting pass walks every row in the
    /// context, which is the whole library on an unfiltered panel, so it
    /// runs on the background executor and the old lists stay up until it
    /// lands. `debounce` waits out the typing burst first.
    fn schedule_refresh(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let (text, filter) = {
            let query = self.state.query.read(cx);
            (query.text().to_string(), query.filter().clone())
        };
        self.rebuild_gen += 1;
        let generation = self.rebuild_gen;
        self.applied_text = text.clone();
        let kinds = self.config.columns.clone();
        // A slot per configured column in everything indexed by the strip,
        // now rather than when the values land: a column added this frame
        // renders before the rebuild comes back, and it renders through
        // these. The column changes carry their own structural edit, so
        // this only ever pads out the lists a first build hasn't filled.
        self.scrolls
            .resize_with(kinds.len(), UniformListScrollHandle::new);
        self.columns.resize_with(kinds.len(), Vec::new);
        self.over_cap.resize(kinds.len(), 0);
        self.pending.resize(kinds.len(), true);
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            self.columns = kinds.iter().map(|_| Vec::new()).collect();
            self.over_cap = kinds.iter().map(|_| 0).collect();
            self.pending = kinds.iter().map(|_| false).collect();
            self.clamp_cursor();
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(REBUILD_DEBOUNCE).await;
                let live = this
                    .update(cx, |this, _| this.rebuild_gen == generation)
                    .unwrap_or(false);
                if !live {
                    return;
                }
            }
            let built = cx
                .background_executor()
                .spawn(async move { build_columns(&projection, &kinds, &text, &filter) })
                .await;
            this.update(cx, |this, cx| {
                if this.rebuild_gen != generation {
                    return;
                }
                let (columns, over_cap): (Vec<_>, Vec<_>) = built.into_iter().unzip();
                this.pending = columns.iter().map(|_| false).collect();
                this.columns = columns;
                this.over_cap = over_cap;
                this.clamp_cursor();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keep the active column and cursor inside the rebuilt lists, so a
    /// rescan or a narrowed context never leaves them pointing off the end.
    fn clamp_cursor(&mut self) {
        let cols = self.config.columns.len();
        if cols == 0 {
            self.active_col = 0;
            self.cursor = None;
            return;
        }
        self.active_col = self.active_col.min(cols - 1);
        if self.cursor.is_some_and(|ix| ix >= self.active_len()) {
            self.cursor = None;
        }
    }

    /// Toggle one value on the shared filter; the Changed subscription
    /// rebuilds the cascade and wakes every follower.
    fn toggle(&mut self, col: usize, value: String, cx: &mut Context<Self>) {
        // The column's values haven't caught up with its field yet, so a
        // value picked out of it belongs to the field that was there a
        // moment ago and would pin a filter nobody asked for.
        if self.pending.get(col).copied().unwrap_or(false) {
            return;
        }
        let Some(&kind) = self.config.columns.get(col) else {
            return;
        };
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            filter.toggle(kind.field(), &value);
            query.set_filter(filter, cx);
        });
    }

    /// The All row: drop every pick for the column's field.
    fn clear_column(&mut self, col: usize, cx: &mut Context<Self>) {
        let Some(&kind) = self.config.columns.get(col) else {
            return;
        };
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            if filter.values(kind.field()).is_empty() {
                return;
            }
            filter.clear(kind.field());
            query.set_filter(filter, cx);
        });
    }

    /// Drop every pick, every field: the panel menu's reset.
    fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.state.query.clone().update(cx, |query, cx| {
            query.set_filter(FilterSet::default(), cx);
        });
    }

    /// The Columns flyout's toggle: on appends the column, off removes
    /// every column of the field along with its picks.
    fn toggle_kind(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        if !self.config.columns.contains(&kind) {
            self.add_column(kind, cx);
            return;
        }
        // Right to left, so the indices ahead of each drop still point at
        // the columns they did when the sweep started.
        for col in (0..self.config.columns.len()).rev() {
            if self.config.columns[col] == kind {
                self.detach_column(col);
            }
        }
        self.drop_picks_if_unused(kind, cx);
        self.clamp_cursor();
        self.refresh(cx);
    }

    fn set_kind(&mut self, col: usize, kind: ColumnKind, cx: &mut Context<Self>) {
        if self.config.columns.get(col) == Some(&kind) {
            return;
        }
        let Some(slot) = self.config.columns.get_mut(col) else {
            return;
        };
        let old = std::mem::replace(slot, kind);
        // The header names the new field from this frame on, so the old
        // field's values go now rather than when the rebuild lands. An
        // empty column for a beat beats a column labelled one thing and
        // listing another.
        if let Some(values) = self.columns.get_mut(col) {
            values.clear();
        }
        if let Some(over) = self.over_cap.get_mut(col) {
            *over = 0;
        }
        if let Some(pending) = self.pending.get_mut(col) {
            *pending = true;
        }
        self.drop_picks_if_unused(old, cx);
        self.clamp_cursor();
        self.refresh(cx);
    }

    fn remove_column(&mut self, col: usize, cx: &mut Context<Self>) {
        let Some(old) = self.detach_column(col) else {
            return;
        };
        self.drop_picks_if_unused(old, cx);
        self.clamp_cursor();
        self.refresh(cx);
    }

    /// Append a column of the field: the + button's and empty state's add.
    /// Twins are allowed, same as a header's kind pick, so the + can stack
    /// a second Album column if you want one.
    fn add_column(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        self.config.columns.push(kind);
        self.columns.push(Vec::new());
        self.over_cap.push(0);
        self.pending.push(true);
        self.scrolls.push(UniformListScrollHandle::new());
        self.refresh(cx);
    }

    /// Move a column to another slot, what a header drop does. Picks come
    /// along untouched since every field keeps its column.
    fn move_column(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.config.columns.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let kind = self.config.columns.remove(from);
        // `to` is the target header's index. After removing `from`, a target
        // that was ahead of it slid back one, so inserting at `to` puts the
        // column past the target on a rightward drag and before it on a
        // leftward one, which is what dropping onto that header should do.
        // The removal also caps `to` at the new length, so the last slot stays
        // reachable.
        let dest = to.min(self.config.columns.len());
        self.config.columns.insert(dest, kind);
        move_slot(&mut self.columns, from, dest);
        move_slot(&mut self.over_cap, from, dest);
        move_slot(&mut self.pending, from, dest);
        move_slot(&mut self.scrolls, from, dest);
        // The keyboard follows the column it was in rather than the slot
        // it used to sit in.
        if self.active_col == from {
            self.active_col = dest;
        } else if from < self.active_col && self.active_col <= dest {
            self.active_col -= 1;
        } else if dest <= self.active_col && self.active_col < from {
            self.active_col += 1;
        }
        self.refresh(cx);
    }

    /// Drop one column out of the config and out of everything indexed by
    /// it, in one step, and hand back the field it held. The caller sheds
    /// the picks and schedules the rebuild; this is the bookkeeping both
    /// removal paths share.
    fn detach_column(&mut self, col: usize) -> Option<ColumnKind> {
        if col >= self.config.columns.len() {
            return None;
        }
        let old = self.config.columns.remove(col);
        remove_slot(&mut self.columns, col);
        remove_slot(&mut self.over_cap, col);
        remove_slot(&mut self.pending, col);
        remove_slot(&mut self.scrolls, col);
        // Everything right of the drop slid one left, the keyboard's
        // column with it; the cursor in the dropped column has nowhere to
        // be.
        if col < self.active_col {
            self.active_col -= 1;
        } else if col == self.active_col {
            self.cursor = None;
        }
        Some(old)
    }

    /// A field that just lost its last column sheds its picks, so a
    /// removed column doesn't keep filtering the app invisibly. A twin
    /// column of the same field keeps them.
    fn drop_picks_if_unused(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        if self.config.columns.contains(&kind) {
            return;
        }
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            if filter.values(kind.field()).is_empty() {
                return;
            }
            filter.clear(kind.field());
            query.set_filter(filter, cx);
        });
    }

    /// One column's header: the field as a plain left-aligned label that
    /// drops the kind pick, clear, and remove, then a grip to reorder by.
    /// The whole header is a drop target, so a column dragged by its grip
    /// can be dropped anywhere on it.
    fn header(&self, col: usize, kind: ColumnKind, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let picked = !self
            .state
            .query
            .read(cx)
            .filter()
            .values(kind.field())
            .is_empty();
        let drag = ColumnDrag {
            from: col,
            label: kind.label().into(),
        };
        div()
            .id(("filter-header", col))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .pr(tokens::SPACE_XS)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .drag_over::<ColumnDrag>(|style, _, _, _| {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            })
            .on_drop(cx.listener(move |this, drag: &ColumnDrag, _, cx| {
                this.move_column(drag.from, col, cx);
            }))
            .child(
                Button::new(("filter-kind", col))
                    .label(kind.label())
                    // Ghost, full width, left-aligned: reads as a plain
                    // heading, not a boxed button, and lines up with the
                    // value rows below. The ghost's own left padding matches
                    // the rows' SPACE_SM.
                    .small()
                    .ghost()
                    .flex_1()
                    .justify_start()
                    .px(tokens::SPACE_SM)
                    .dropdown_menu(move |mut menu, _, _| {
                        for pick in ColumnKind::ALL {
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(pick.label())
                                    .checked(pick == kind)
                                    .on_click(move |_, _, cx| {
                                        let Some(this) = weak.upgrade() else { return };
                                        this.update(cx, |this, cx| this.set_kind(col, pick, cx));
                                    }),
                            );
                        }
                        let clear = weak.clone();
                        let remove = weak.clone();
                        menu.separator()
                            .item(
                                PopupMenuItem::new(rox_i18n::t!("filter-clear-selection"))
                                    .icon(Icon::default().path(icons::CLOSE))
                                    .disabled(!picked)
                                    .on_click(move |_, _, cx| {
                                        let Some(this) = clear.upgrade() else { return };
                                        this.update(cx, |this, cx| this.clear_column(col, cx));
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(rox_i18n::t!("filter-remove-column"))
                                    .icon(Icon::default().path(icons::TRASH))
                                    .on_click(move |_, _, cx| {
                                        let Some(this) = remove.upgrade() else { return };
                                        this.update(cx, |this, cx| this.remove_column(col, cx));
                                    }),
                            )
                    }),
            )
            .child(
                div()
                    .id(("filter-grip", col))
                    .flex_none()
                    .flex()
                    .items_center()
                    .cursor_grab()
                    .text_color(palette::text_faint())
                    .hover(|d| d.text_color(palette::text_muted()))
                    .on_drag(drag, |drag, _pos, _window, cx| {
                        cx.new(|_| ColumnDragPreview {
                            label: drag.label.clone(),
                        })
                    })
                    .child(svg().path(icons::MOVE_HORIZONTAL).size(px(12.))),
            )
    }

    /// The add-column control: a + that drops a menu of every field. Shown
    /// trailing the strip and, labelled, in the empty state, so a column
    /// can be added without the panel menu. Twins are fine here, matching a
    /// header's kind pick.
    fn add_button(&self, labelled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let button = Button::new("filter-add")
            .icon(Icon::default().path(icons::PLUS))
            .small();
        let button = if labelled {
            button.label(rox_i18n::t!("filter-add-column")).outline()
        } else {
            button
                .ghost()
                .tooltip(rox_i18n::t!("filter-add-column-tooltip"))
        };
        button.dropdown_menu(move |mut menu, _, _| {
            for kind in ColumnKind::ALL {
                let weak = weak.clone();
                menu = menu.item(PopupMenuItem::new(kind.label()).on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.add_column(kind, cx));
                }));
            }
            menu
        })
    }

    /// The fixed All row over a column's list: the whole field, picked
    /// style while nothing narrows it, a click back to it.
    fn all_row(&self, col: usize, cx: &mut Context<Self>) -> Div {
        let picked = self
            .config
            .columns
            .get(col)
            .map(|kind| {
                !self
                    .state
                    .query
                    .read(cx)
                    .filter()
                    .values(kind.field())
                    .is_empty()
            })
            .unwrap_or(false);
        let distinct = self.columns.get(col).map(Vec::len).unwrap_or(0);
        div()
            .flex_none()
            .w_full()
            .h(palette::scaled_px(ROW_H))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(!picked, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.clear_column(col, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(rox_i18n::t!("filter-all")),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rox_i18n::format::format_int(
                        distinct as i64,
                    ))),
            )
    }

    /// The line under a column that held more values than it lists: what
    /// the cap left out, and the way to see it. Sits below the list rather
    /// than in it, so the row indices the cursor and the type-ahead walk
    /// stay the values' own.
    fn over_cap_row(&self, col: usize) -> Option<Div> {
        let dropped = *self.over_cap.get(col)?;
        if dropped == 0 {
            return None;
        }
        Some(
            div()
                .flex_none()
                .w_full()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .border_t_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .truncate()
                .child(rox_i18n::t!("filter-over-cap", count = dropped as u64)),
        )
    }

    /// The visible slice of one column's list.
    fn list_rows(
        &mut self,
        col: usize,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let Some(values) = self.columns.get(col) else {
            return Vec::new();
        };
        let cursor = (col == self.active_col).then_some(self.cursor).flatten();
        range
            .filter_map(|ix| {
                let value = values.get(ix)?;
                let pick = value.value.clone();
                Some(
                    div()
                        .w_full()
                        .h(palette::scaled_px(ROW_H))
                        .px(tokens::SPACE_SM)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .cursor_pointer()
                        .when(value.selected, |d| {
                            d.bg(palette::alpha(palette::accent(), 0x26))
                        })
                        // The keyboard cursor: a faint outline so it reads as
                        // "where typing went" without stealing the picked
                        // rows' fill.
                        .when(cursor == Some(ix), |d| {
                            d.border_1().border_color(palette::accent())
                        })
                        .hover(|d| d.bg(palette::bg_control_hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                window.focus(&this.focus);
                                this.active_col = col;
                                this.cursor = Some(ix);
                                this.type_ahead.clear();
                                this.toggle(col, pick.clone(), cx);
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(value.label.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(SharedString::from(rox_i18n::format::format_int(
                                    value.count as i64,
                                ))),
                        ),
                )
            })
            .collect()
    }

    /// The column toggles: one row per filter field, ticked while a
    /// column shows it, the library's header-menu toggles' shape. Flat
    /// top-level rows under a label, not a flyout: a submenu built from a
    /// panel's `dropdown_menu` runs in the panel's context, so it can't
    /// wire the parent link the `PopupMenu::submenu` builder sets, and a
    /// leaf click would dead-end there: the tab-owned root never gets
    /// the dismiss and the menu hangs open with its checks frozen. Flat
    /// rows dismiss the root cleanly, so the next open reads the change. A
    /// twin column made through a header's kind pick still counts as
    /// shown.
    fn columns_menu(&self, mut menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let shown = self.config.columns.clone();
        menu = menu.label(rox_i18n::t!("library-columns"));
        for kind in ColumnKind::ALL {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(kind.label())
                    .checked(shown.contains(&kind))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| this.toggle_kind(kind, cx));
                    }),
            );
        }
        menu
    }
}

impl PanelSettings for FilterPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for FilterPanel {}

impl Focusable for FilterPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for FilterPanel {
    fn panel_name(&self) -> &'static str {
        "filter"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("content-filter"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = self.columns_menu(menu, cx);
        let filtering = !self.state.query.read(cx).filter().is_empty();
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("filter-clear-filters"))
                .icon(Icon::default().path(icons::CLOSE))
                .disabled(!filtering)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.clear_all(cx));
                }),
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                FilterPanel::new(state, config, window, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for FilterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl FilterPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // Scopes the workspace's space-bound playback binding out while
            // a type-ahead phrase is mid-flight, the same way the search
            // box's own context does: bindings win over key listeners, so
            // without this a space continuing a phrase would also toggle
            // playback before on_panel_key ever saw the keystroke.
            // While a phrase is up the panel carries its contexts, which
            // scope the workspace's space binding out (only while the
            // phrase is still taking keystrokes) and Root's tab traversal
            // out (for as long as there's a phrase to cycle).
            .when_some(
                panel::type_ahead_context(&self.type_ahead, self.type_ahead_at),
                |d, context| d.key_context(context),
            )
            // A press anywhere in the panel ends the phrase: the cursor
            // has moved by hand, so the cycle it was stepping is stale,
            // and tab belongs back with panel traversal. Capture phase,
            // so rows and tiles that stop the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            // Tab cycles the live phrase's matches, off the bindings the
            // TypeAhead context above scopes in; with no phrase up, tab
            // stays Root's focus traversal.
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_panel_key(event, cx)),
            );
        if self.config.columns.is_empty() {
            return root.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(tokens::SPACE_MD)
                    .child(
                        div()
                            .text_color(palette::text_faint())
                            .child(rox_i18n::t!("filter-empty")),
                    )
                    .child(self.add_button(true, cx)),
            );
        }
        let mut cols = div().flex_1().min_h_0().flex().flex_row();
        for (col, &kind) in self.config.columns.clone().iter().enumerate() {
            let count = self.columns.get(col).map(Vec::len).unwrap_or(0);
            let this = cx.entity().downgrade();
            cols = cols.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when(col > 0, |d| d.border_l_1().border_color(palette::border()))
                    .child(self.header(col, kind, cx))
                    .child(self.all_row(col, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .relative()
                            .child(
                                uniform_list(("filter-values", col), count, move |range, _, cx| {
                                    this.upgrade()
                                        .map(|this| {
                                            this.update(cx, |this, cx| {
                                                this.list_rows(col, range, cx)
                                            })
                                        })
                                        .unwrap_or_default()
                                })
                                .track_scroll(self.scrolls[col].clone())
                                .size_full(),
                            )
                            .child(
                                div().absolute().inset_0().child(
                                    // Scrollbar ids default to the call site, so
                                    // every column would share one; key by column.
                                    Scrollbar::vertical(&self.scrolls[col])
                                        .id(("filter-scrollbar", col)),
                                ),
                            ),
                    )
                    .children(self.over_cap_row(col)),
            );
        }
        // The trailing add rail: a slim column whose header cell holds the +,
        // so more fields go on without the panel menu.
        cols = cols.child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(palette::border())
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(tokens::SPACE_XS)
                        .py(tokens::SPACE_XS)
                        .border_b_1()
                        .border_color(palette::border())
                        .child(self.add_button(false, cx)),
                ),
        );
        root.child(cols.relative().children(panel::type_ahead_overlay(
            &self.type_ahead,
            self.type_ahead_at,
        )))
    }
}

/// The rows a column counts over. The unqueried case is every live row in
/// the library, and naming it beats materializing it: `(0..len).collect()`
/// on a ten-million-row projection is forty megabytes allocated per
/// rebuild to hold the numbers zero through ten million. It carries the
/// projection rather than a length so the walk can skip the tombstones a
/// patch left behind; every other way in here already excludes them.
enum RowSet<'a> {
    All(&'a Projection),
    Only(Vec<u32>),
}

impl RowSet<'_> {
    /// One counter per symbol over the set, in parallel chunks: the rows
    /// far outnumber the symbols, so each chunk tallies into its own
    /// counters and the chunks' sums fold together at the end.
    fn count_with(&self, symbols: usize, sym: impl Fn(usize) -> usize + Sync) -> Vec<u32> {
        let empty = || vec![0u32; symbols];
        let merge = |mut a: Vec<u32>, b: Vec<u32>| {
            for (slot, count) in a.iter_mut().zip(b) {
                *slot += count;
            }
            a
        };
        match self {
            RowSet::All(projection) => (0..projection.len())
                .into_par_iter()
                .with_min_len(COUNT_CHUNK)
                .fold(empty, |mut acc, row| {
                    if !projection.is_dead(row as u32) {
                        acc[sym(row)] += 1;
                    }
                    acc
                })
                .reduce(empty, merge),
            RowSet::Only(rows) => rows
                .par_iter()
                .with_min_len(COUNT_CHUNK)
                .fold(empty, |mut acc, &row| {
                    acc[sym(row as usize)] += 1;
                    acc
                })
                .reduce(empty, merge),
        }
    }

    /// The set narrowed by a mask, the cascade's step between columns.
    fn narrow(self, mask: &[bool]) -> Self {
        match self {
            // The mask is false at every tombstone, so this drops them
            // with the rows the filter rules out.
            RowSet::All(projection) => RowSet::Only(
                (0..projection.len() as u32)
                    .into_par_iter()
                    .with_min_len(COUNT_CHUNK)
                    .filter(|&row| mask[row as usize])
                    .collect(),
            ),
            RowSet::Only(mut rows) => {
                rows.retain(|&row| mask[row as usize]);
                RowSet::Only(rows)
            }
        }
    }
}

/// Every column's values in one pass, left to right: the context starts as
/// the text query's hits and each column's own picks narrow it for the
/// columns after it. Blocking and allocation-heavy over a big library, so
/// it runs off the UI thread; nothing in it touches a window or an entity.
fn build_columns(
    projection: &Projection,
    kinds: &[ColumnKind],
    text: &str,
    filter: &FilterSet,
) -> Vec<(Vec<Value>, usize)> {
    let mut rows = if text.is_empty() {
        RowSet::All(projection)
    } else {
        RowSet::Only(projection.search(text))
    };
    let mut out = Vec::with_capacity(kinds.len());
    for (ix, &kind) in kinds.iter().enumerate() {
        let picks = filter.values(kind.field());
        out.push(column_values(projection, kind, &rows, picks));
        // The last column's picks narrow nothing, since no column reads
        // the context after it.
        if picks.is_empty() || ix + 1 == kinds.len() {
            continue;
        }
        let mut sub = FilterSet::default();
        sub.fields.push((kind.field(), picks.to_vec()));
        if let Some(mask) = projection.filter_mask(&sub) {
            rows = rows.narrow(&mask);
        }
    }
    out
}

/// One column's value rows out of its context: every distinct value with
/// its track count, alphabetical for the interned fields, ascending for
/// years. A pick whose value fell out of the context (the text query
/// moved on) stays listed at zero so it can still be cleared. Comes back
/// with how many values [`VALUE_CAP`] left out, zero when it listed
/// everything.
fn column_values(
    projection: &Projection,
    kind: ColumnKind,
    rows: &RowSet<'_>,
    picks: &[String],
) -> (Vec<Value>, usize) {
    let out = match kind {
        // A year is its own symbol: two bytes wide, so one counter per
        // possible year is a quarter of a megabyte and the values come out
        // in order with no sort behind them.
        ColumnKind::Year => {
            let counts = rows.count_with(u16::MAX as usize + 1, |i| projection.year[i] as usize);
            counts
                .into_iter()
                .enumerate()
                .filter(|&(_, count)| count > 0)
                .map(|(year, count)| {
                    let year = year as u16;
                    let value = year.to_string();
                    Value {
                        label: year_label(year),
                        selected: picks.iter().any(|p| p == &value),
                        value,
                        count,
                    }
                })
                .collect::<Vec<_>>()
        }
        // Genre symbols are "; " lists, and the column lists their
        // values: counts aggregate per symbol first (cheap over rows),
        // then fan out onto each symbol's values. A symbol with no
        // values at all is the untagged bucket, the "" row. A folded
        // library merges case variants here too (the symbols only
        // folded whole lists), showing the casing most rows use.
        ColumnKind::Genre => {
            let fold = crate::settings::fold_case();
            let (column, table) = sym_source(projection, kind);
            let sym_counts = rows.count_with(table.strings.len(), |i| column[i] as usize);
            // (Folded) value -> per-casing counts, so the display can
            // follow the rows once every symbol has fanned out.
            let mut counts: HashMap<String, HashMap<String, u32>> = HashMap::new();
            for (sym, &count) in sym_counts.iter().enumerate() {
                if count == 0 {
                    continue;
                }
                // Aliases first, then dedup within one symbol, so a
                // degenerate "Rock; Rock" (or "Rock; rock" folded, or an
                // alias pair) still counts its tracks once.
                let mut parts: Vec<String> = rox_library::genre::split(&table.strings[sym])
                    .map(rox_library::genre::resolve)
                    .collect();
                if fold {
                    parts.sort_unstable_by_key(|p| p.to_lowercase());
                    parts.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
                } else {
                    parts.sort_unstable();
                    parts.dedup();
                }
                if parts.is_empty() {
                    parts.push(String::new());
                }
                for part in parts {
                    let key = if fold {
                        part.to_lowercase()
                    } else {
                        part.clone()
                    };
                    *counts.entry(key).or_default().entry(part).or_default() += count;
                }
            }
            let mut values: Vec<(String, u32)> = counts
                .into_values()
                .map(|casings| {
                    let total = casings.values().sum();
                    let display = casings
                        .into_iter()
                        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                        .map(|(s, _)| s.to_string())
                        .unwrap_or_default();
                    (display, total)
                })
                .collect();
            values.sort_unstable_by_key(|(value, _)| value.to_lowercase());
            values
                .into_iter()
                .map(|(value, count)| Value {
                    label: sym_label(&value),
                    selected: picks.iter().any(|p| rox_library::value_eq(p, &value, fold)),
                    value,
                    count,
                })
                .collect()
        }
        _ => {
            let (column, table) = sym_source(projection, kind);
            let counts = rows.count_with(table.strings.len(), |i| column[i] as usize);
            let mut syms: Vec<u32> = (0..counts.len() as u32)
                .filter(|&sym| counts[sym as usize] > 0)
                .collect();
            syms.sort_unstable_by(|&a, &b| table.lower[a as usize].cmp(&table.lower[b as usize]));
            syms.into_iter()
                .map(|sym| {
                    let value = table.strings[sym as usize].clone();
                    Value {
                        label: sym_label(&value),
                        selected: picks.iter().any(|p| p == &value),
                        count: counts[sym as usize],
                        value,
                    }
                })
                .collect()
        }
    };
    let (mut out, dropped) = cap_values(out, VALUE_CAP);
    for pick in picks {
        if !out.iter().any(|value| &value.value == pick) {
            let label = match kind {
                ColumnKind::Year => pick
                    .parse::<u16>()
                    .map(year_label)
                    .unwrap_or_else(|_| SharedString::from(pick.clone())),
                _ => sym_label(pick),
            };
            out.push(Value {
                label,
                value: pick.clone(),
                count: 0,
                selected: true,
            });
        }
    }
    (out, dropped)
}

/// A column's values cut down to `cap`, keeping the ones the most tracks
/// carry and leaving the rest in the order they came in. Comes back with
/// how many it dropped. Picked values are never dropped: a pick the panel
/// stopped listing is a filter nothing on screen could clear.
fn cap_values(values: Vec<Value>, cap: usize) -> (Vec<Value>, usize) {
    if values.len() <= cap {
        return (values, 0);
    }
    let mut ranked: Vec<usize> = (0..values.len()).filter(|&i| !values[i].selected).collect();
    ranked.sort_unstable_by(|&a, &b| values[b].count.cmp(&values[a].count));
    let dropped = ranked.len().saturating_sub(cap);
    let cut: HashSet<usize> = ranked.into_iter().skip(cap).collect();
    let kept = values
        .into_iter()
        .enumerate()
        .filter(|(ix, _)| !cut.contains(ix))
        .map(|(_, value)| value)
        .collect();
    (kept, dropped)
}

/// The interned column and table one kind reads; years go their own way.
fn sym_source(projection: &Projection, kind: ColumnKind) -> (&[u32], &SymTable) {
    match kind {
        ColumnKind::Artist => (&projection.artist, &projection.artists),
        ColumnKind::AlbumArtist => (&projection.album_artist, &projection.album_artists),
        ColumnKind::Album => (&projection.album, &projection.albums),
        ColumnKind::Genre => (&projection.genre, &projection.genres),
        ColumnKind::Year => unreachable!("years don't intern"),
    }
}

/// An untagged value shows as Unknown but filters as its real empty
/// string, so the pick still matches exactly.
fn sym_label(value: &str) -> SharedString {
    if value.is_empty() {
        rox_i18n::t!("filter-unknown")
    } else {
        SharedString::from(value.to_string())
    }
}

/// Year zero is the untagged marker, the scanner's default.
fn year_label(year: u16) -> SharedString {
    if year == 0 {
        rox_i18n::t!("filter-unknown")
    } else {
        SharedString::from(year.to_string())
    }
}

/// Drop the slot a removed column held out of one of the lists indexed
/// by the column strip. The lists are only as long as the last rebuild
/// made them, so an index past the end is a column whose values never
/// landed and there's nothing to drop.
fn remove_slot<T>(slots: &mut Vec<T>, ix: usize) {
    if ix < slots.len() {
        slots.remove(ix);
    }
}

/// Carry one slot from `from` to `dest`, the same walk the config's own
/// columns take on a header drop, so a moved column keeps its values, its
/// cap notice and its scroll position.
fn move_slot<T>(slots: &mut Vec<T>, from: usize, dest: usize) {
    if from >= slots.len() {
        return;
    }
    let slot = slots.remove(from);
    let dest = dest.min(slots.len());
    slots.insert(dest, slot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::{store, TrackRow};

    fn track(path: &str, artist: &str, year: u16) -> TrackRow {
        TrackRow {
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            sub: 0,
            cue: None,
            path: path.into(),
            title: path.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: "Album".into(),
            genre: "Rock".into(),
            year,
            disc_no: 1,
            track_no: 1,
            duration_ms: 1000,
            codec: "flac".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44100,
            bit_depth: 16,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    fn read(values: &[Value]) -> Vec<(String, u32)> {
        values.iter().map(|v| (v.value.clone(), v.count)).collect()
    }

    /// Naming the whole library instead of listing it counts the same
    /// values with the same totals, for the interned columns and for
    /// years, which count their own way.
    #[test]
    fn the_all_rows_sentinel_counts_what_the_listed_rows_do() {
        let p = projection(&[
            track("/m/1.flac", "A", 1999),
            track("/m/2.flac", "B", 2001),
            track("/m/3.flac", "A", 2001),
            track("/m/4.flac", "C", 0),
        ]);
        let listed = RowSet::Only((0..p.len() as u32).collect());
        let all = RowSet::All(&p);
        for kind in [ColumnKind::Artist, ColumnKind::Album, ColumnKind::Year] {
            let (from_all, over) = column_values(&p, kind, &all, &[]);
            let (from_listed, _) = column_values(&p, kind, &listed, &[]);
            assert_eq!(read(&from_all), read(&from_listed), "{:?}", kind.label());
            assert_eq!(over, 0);
        }
    }

    /// A row a patch tombstoned counts for nothing. The sentinel walks
    /// the columns by index, so it's the one way into the counts that
    /// has to check liveness itself.
    #[test]
    fn the_all_rows_sentinel_skips_tombstoned_rows() {
        let mut p = projection(&[
            track("/m/1.flac", "A", 1999),
            track("/m/2.flac", "B", 2001),
            track("/m/3.flac", "A", 2001),
        ]);
        let index: HashMap<i64, u32> = p
            .db_id
            .iter()
            .enumerate()
            .map(|(row, &id)| (id, row as u32))
            .collect();
        let gone = p.db_id[2];
        p.remove_ids(&[gone], &index);
        let all = RowSet::All(&p);
        let (artists, _) = column_values(&p, ColumnKind::Artist, &all, &[]);
        assert_eq!(read(&artists), vec![("A".into(), 1), ("B".into(), 1)]);
        let (years, _) = column_values(&p, ColumnKind::Year, &all, &[]);
        assert_eq!(read(&years), vec![("1999".into(), 1), ("2001".into(), 1)]);
    }

    /// Over the cap a column keeps the values the most tracks carry, in
    /// the order it built them, and says how many it left out.
    #[test]
    fn the_cap_keeps_the_biggest_values_in_place() {
        let value = |name: &str, count: u32, selected: bool| Value {
            label: name.to_string().into(),
            value: name.to_string(),
            count,
            selected,
        };
        let (kept, dropped) = cap_values(
            vec![
                value("a", 1, false),
                value("b", 9, false),
                value("c", 4, false),
                value("d", 2, false),
            ],
            2,
        );
        assert_eq!(dropped, 2);
        assert_eq!(read(&kept), vec![("b".into(), 9), ("c".into(), 4)]);
    }

    /// The lists indexed by the column strip take the same edit the
    /// config's columns do, so a dropped or moved column takes its values
    /// with it instead of leaving everything right of it off by one.
    #[test]
    fn the_value_lists_follow_the_columns_they_belong_to() {
        let pairs = |columns: &[&'static str], values: &[&'static str]| {
            columns
                .iter()
                .copied()
                .zip(values.iter().copied())
                .collect::<Vec<_>>()
        };
        let mut columns = vec!["artist", "album", "genre", "year"];
        let mut values = vec!["a", "b", "g", "y"];

        // A drop out of the middle.
        columns.remove(1);
        remove_slot(&mut values, 1);
        assert_eq!(
            pairs(&columns, &values),
            vec![("artist", "a"), ("genre", "g"), ("year", "y")]
        );

        // A header drop rightward, walked the way `move_column` walks
        // the config: past the target.
        let kind = columns.remove(0);
        let dest = 2.min(columns.len());
        columns.insert(dest, kind);
        move_slot(&mut values, 0, dest);
        assert_eq!(
            pairs(&columns, &values),
            vec![("genre", "g"), ("year", "y"), ("artist", "a")]
        );

        // And leftward, back where it came from. The clamp `move_column`
        // applies is a no-op at the head, so the drop index is the target's.
        let kind = columns.remove(2);
        columns.insert(0, kind);
        move_slot(&mut values, 2, 0);
        assert_eq!(
            pairs(&columns, &values),
            vec![("artist", "a"), ("genre", "g"), ("year", "y")]
        );

        // A column added before the first rebuild has no list yet, so an
        // edit past the end is a no-op rather than a panic.
        let mut unbuilt: Vec<&str> = Vec::new();
        remove_slot(&mut unbuilt, 2);
        move_slot(&mut unbuilt, 1, 0);
        assert!(unbuilt.is_empty());
    }

    /// A picked value survives the cap however few tracks carry it, or
    /// the pick would be a filter with no row left to clear it from.
    #[test]
    fn the_cap_never_drops_a_pick() {
        let value = |name: &str, count: u32, selected: bool| Value {
            label: name.to_string().into(),
            value: name.to_string(),
            count,
            selected,
        };
        let (kept, dropped) = cap_values(
            vec![
                value("a", 1, true),
                value("b", 9, false),
                value("c", 4, false),
            ],
            1,
        );
        assert_eq!(dropped, 1);
        assert_eq!(read(&kept), vec![("a".into(), 1), ("b".into(), 9)]);
    }
}
