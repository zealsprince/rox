//! The filter panel: the library's field values as cascading columns, each
//! listing every distinct value with its track count. Picking values writes
//! the shared query's structured filter, so every global-following panel
//! narrows with it, and each column narrows by the picks left of it. Columns
//! are per-panel config; the picks are the one app-wide filter, so two
//! filter panels share them.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use gpui::{
    App, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    WeakEntity, Window, div, prelude::*, px, svg, uniform_list,
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

const ROW_H: f32 = 26.;

/// The most values one column lists. Past this the values cost real memory
/// for a list nobody reads to the end, so the column keeps the most-used and
/// says how many it left out.
const VALUE_CAP: usize = 5000;

/// The per-row work is an index and an increment, so the chunk has to be big
/// or the split costs more than the pass. Each chunk holds its own counters.
const COUNT_CHUNK: usize = 256 * 1024;

/// Picks don't wait: the row a click lit up should fill in on the same
/// frame.
const REBUILD_DEBOUNCE: Duration = Duration::from_millis(100);

/// How long a type-ahead phrase keeps growing between keystrokes.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    Artist,
    AlbumArtist,
    Album,
    Genre,
    Year,
    /// The folders on disk as one value, and each server as its own.
    Source,
}

impl ColumnKind {
    pub const ALL: [ColumnKind; 6] = [
        ColumnKind::Artist,
        ColumnKind::AlbumArtist,
        ColumnKind::Album,
        ColumnKind::Genre,
        ColumnKind::Year,
        ColumnKind::Source,
    ];

    fn label(self) -> &'static str {
        match self {
            ColumnKind::Artist => rox_i18n::t_static("filter-field-artist"),
            ColumnKind::AlbumArtist => rox_i18n::t_static("filter-field-album-artist"),
            ColumnKind::Album => rox_i18n::t_static("filter-field-album"),
            ColumnKind::Genre => rox_i18n::t_static("filter-field-genre"),
            ColumnKind::Year => rox_i18n::t_static("filter-field-year"),
            ColumnKind::Source => rox_i18n::t_static("filter-field-source"),
        }
    }

    fn field(self) -> FilterField {
        match self {
            ColumnKind::Artist => FilterField::Artist,
            ColumnKind::AlbumArtist => FilterField::AlbumArtist,
            ColumnKind::Album => FilterField::Album,
            ColumnKind::Genre => FilterField::Genre,
            ColumnKind::Year => FilterField::Year,
            ColumnKind::Source => FilterField::Source,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
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

struct Value {
    label: SharedString,
    value: String,
    count: u32,
    selected: bool,
}

#[derive(Clone)]
struct ColumnDrag {
    from: usize,
    label: SharedString,
}

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
    columns: Vec<Vec<Value>>,
    /// Per column: how many values [`VALUE_CAP`] left out.
    over_cap: Vec<usize>,
    /// Per column: whether its values are a rebuild behind its field, so a
    /// click can tell a waiting list from an empty one.
    pending: Vec<bool>,
    rebuild_gen: u64,
    /// The query text the lists were built from, so a change can tell typing
    /// (which waits for the pause) from a pick (which doesn't).
    applied_text: String,
    scrolls: Vec<UniformListScrollHandle>,
    /// The column the keyboard drives, set by clicking a value.
    active_col: usize,
    cursor: Option<usize>,
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
    /// Drops the phrase when focus leaves, so tab goes back to walking
    /// panels.
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
        // The picks arrive here too. Only the typed text waits out the
        // debounce.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, query, _: &SharedQueryEvent, cx| {
                let typed = query.read(cx).text() != this.applied_text;
                this.schedule_refresh(typed, cx);
            },
        );
        let focus = cx.focus_handle().tab_stop(true);
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

    /// Left and right stay the workspace's seek, and space its play/pause
    /// unless a phrase is mid-flight.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        match keystroke.key.as_str() {
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
                if let Some(ix) = self.cursor
                    && let Some(value) = self.value_at(self.active_col, ix)
                {
                    self.toggle(self.active_col, value, cx);
                }
            }
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                // The phrase never clears on its own, so gate space on the
                // type-ahead window, not on the phrase being empty.
                let phrase_live =
                    !self.type_ahead.is_empty() && panel::type_ahead_live(self.type_ahead_at);
                if !phrase_live && text == " " {
                    return;
                }
                // Stop it so it doesn't also fire the workspace's space-bound
                // TogglePlayback, which this panel inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

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
        // A miss below still updated the badge, so repaint either way.
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

    /// True when there was a phrase, for the escape ladder.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Leaves the window stamp alone, so a run of tabs steps silently rather
    /// than reviving the badge.
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

    fn active_len(&self) -> usize {
        self.columns.get(self.active_col).map(Vec::len).unwrap_or(0)
    }

    fn value_at(&self, col: usize, ix: usize) -> Option<String> {
        self.columns.get(col)?.get(ix).map(|v| v.value.clone())
    }

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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.schedule_refresh(false, cx);
    }

    /// The counting pass walks the whole library on an unfiltered panel, so
    /// it runs in the background and the old lists stay up until it lands.
    fn schedule_refresh(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let (text, filter) = {
            let query = self.state.query.read(cx);
            (query.text().to_string(), query.filter().clone())
        };
        self.rebuild_gen += 1;
        let generation = self.rebuild_gen;
        self.applied_text = text.clone();
        let kinds = self.config.columns.clone();
        // A slot per column now, not when the values land: a column added
        // this frame renders before the rebuild comes back.
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

    fn toggle(&mut self, col: usize, value: String, cx: &mut Context<Self>) {
        // A column a rebuild behind still lists the old field's values, so a
        // pick out of it would pin a filter nobody asked for.
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

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        self.state.query.clone().update(cx, |query, cx| {
            query.set_filter(FilterSet::default(), cx);
        });
    }

    fn toggle_kind(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        if !self.config.columns.contains(&kind) {
            self.add_column(kind, cx);
            return;
        }
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
        // field's values go now.
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

    /// Twins are allowed, same as a header's kind pick.
    fn add_column(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        self.config.columns.push(kind);
        self.columns.push(Vec::new());
        self.over_cap.push(0);
        self.pending.push(true);
        self.scrolls.push(UniformListScrollHandle::new());
        self.refresh(cx);
    }

    fn move_column(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.config.columns.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let kind = self.config.columns.remove(from);
        // With `from` removed, inserting at `to` lands past the target on a
        // rightward drag and before it on a leftward one. The min keeps the
        // last slot reachable.
        let dest = to.min(self.config.columns.len());
        self.config.columns.insert(dest, kind);
        move_slot(&mut self.columns, from, dest);
        move_slot(&mut self.over_cap, from, dest);
        move_slot(&mut self.pending, from, dest);
        move_slot(&mut self.scrolls, from, dest);
        // The keyboard follows the column, not the slot.
        if self.active_col == from {
            self.active_col = dest;
        } else if from < self.active_col && self.active_col <= dest {
            self.active_col -= 1;
        } else if dest <= self.active_col && self.active_col < from {
            self.active_col += 1;
        }
        self.refresh(cx);
    }

    /// The bookkeeping both removal paths share. The caller sheds the picks
    /// and schedules the rebuild.
    fn detach_column(&mut self, col: usize) -> Option<ColumnKind> {
        if col >= self.config.columns.len() {
            return None;
        }
        let old = self.config.columns.remove(col);
        remove_slot(&mut self.columns, col);
        remove_slot(&mut self.over_cap, col);
        remove_slot(&mut self.pending, col);
        remove_slot(&mut self.scrolls, col);
        if col < self.active_col {
            self.active_col -= 1;
        } else if col == self.active_col {
            self.cursor = None;
        }
        Some(old)
    }

    /// So a removed column doesn't keep filtering the app invisibly. A twin
    /// column of the same field keeps the picks.
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

    fn header(
        &self,
        col: usize,
        kind: ColumnKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                    // Ghost and left-aligned, so it reads as a plain heading
                    // lined up with the value rows.
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

    fn add_button(&self, labelled: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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

    /// Below the list rather than in it, so the row indices the cursor and
    /// the type-ahead walk stay the values' own.
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
                        // An outline, so the cursor doesn't steal the picked
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

    /// Flat rows under a label, never a flyout: a submenu built from a
    /// panel's `dropdown_menu` can't wire the parent link, so a leaf click
    /// never dismisses the root and the menu hangs open with its checks frozen.
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
            // Bindings win over key listeners, so while a phrase is up the
            // panel carries contexts that scope out the workspace's space
            // binding and Root's tab traversal.
            .when_some(
                panel::type_ahead_context(&self.type_ahead, self.type_ahead_at),
                |d, context| d.key_context(context),
            )
            // A press anywhere ends the phrase. Capture phase, so rows that
            // stop the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            // Tab cycles the phrase's matches, off the bindings the TypeAhead
            // context scopes in.
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
                    .p(tokens::SPACE_MD)
                    .text_center()
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

/// Naming the whole library beats materializing it: `(0..len).collect()` on
/// a ten-million-row projection is forty megabytes per rebuild. `All` keeps
/// the projection so the walk can skip rows that don't browse.
enum RowSet<'a> {
    All(&'a Projection),
    Only(Vec<u32>),
}

impl RowSet<'_> {
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
                    if projection.is_browsable(row as u32) {
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

    fn narrow(self, mask: &[bool]) -> Self {
        match self {
            // The mask is false at every tombstone and every station, so
            // this drops them too.
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

/// The context starts as the text query's hits, and each column's picks
/// narrow it for the columns after it.
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

/// A pick whose value fell out of the context stays listed at zero so it can
/// still be cleared.
fn column_values(
    projection: &Projection,
    kind: ColumnKind,
    rows: &RowSet<'_>,
    picks: &[String],
) -> (Vec<Value>, usize) {
    let out = match kind {
        // One counter per possible year is a quarter of a megabyte, and the
        // values come out in order with no sort.
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
        // Genre symbols are "; " lists: counts aggregate per symbol, then fan
        // out onto each symbol's values. A folded library merges case
        // variants here, showing the casing most rows use.
        ColumnKind::Genre => {
            let fold = crate::settings::fold_case();
            let (column, table) = sym_source(projection, kind);
            let sym_counts = rows.count_with(table.strings.len(), |i| column[i] as usize);
            let mut counts: HashMap<String, HashMap<String, u32>> = HashMap::new();
            for (sym, &count) in sym_counts.iter().enumerate() {
                if count == 0 {
                    continue;
                }
                // Aliases first, then dedup, so "Rock; Rock" or an alias pair
                // still counts its tracks once.
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
        // Picked by the stored string, which for a server is a digest, so the
        // order and the labels come off the name.
        ColumnKind::Source => {
            let (column, table) = sym_source(projection, kind);
            let counts = rows.count_with(table.strings.len(), |i| column[i] as usize);
            let mut values: Vec<(String, String, u32)> = (0..counts.len())
                .filter(|&sym| counts[sym] > 0)
                .map(|sym| {
                    let value = table.strings[sym].clone();
                    let label = rox_library::cue::source_label(&value);
                    (value, label, counts[sym])
                })
                .collect();
            values.sort_unstable_by_key(|(_, label, _)| label.to_lowercase());
            values
                .into_iter()
                .map(|(value, label, count)| Value {
                    label: SharedString::from(label),
                    selected: picks.iter().any(|p| p == &value),
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
                ColumnKind::Source => SharedString::from(rox_library::cue::source_label(pick)),
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

/// Picked values are never dropped: a pick the panel stopped listing is a
/// filter nothing on screen could clear.
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

fn sym_source(projection: &Projection, kind: ColumnKind) -> (&[u32], &SymTable) {
    match kind {
        ColumnKind::Artist => (&projection.artist, &projection.artists),
        ColumnKind::AlbumArtist => (&projection.album_artist, &projection.album_artists),
        ColumnKind::Album => (&projection.album, &projection.albums),
        ColumnKind::Genre => (&projection.genre, &projection.genres),
        ColumnKind::Source => (&projection.source, &projection.sources),
        ColumnKind::Year => unreachable!("years don't intern"),
    }
}

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

/// An index past the end is a column whose values never landed.
fn remove_slot<T>(slots: &mut Vec<T>, ix: usize) {
    if ix < slots.len() {
        slots.remove(ix);
    }
}

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
    use rox_library::{TrackRow, store};

    fn track(path: &str, artist: &str, year: u16) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
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

    /// The sentinel walks the columns by index, so it's the one way into the
    /// counts that has to check liveness itself.
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

        columns.remove(1);
        remove_slot(&mut values, 1);
        assert_eq!(
            pairs(&columns, &values),
            vec![("artist", "a"), ("genre", "g"), ("year", "y")]
        );

        // A header drop rightward, walked the way `move_column` walks it.
        let kind = columns.remove(0);
        let dest = 2.min(columns.len());
        columns.insert(dest, kind);
        move_slot(&mut values, 0, dest);
        assert_eq!(
            pairs(&columns, &values),
            vec![("genre", "g"), ("year", "y"), ("artist", "a")]
        );

        // And leftward, back where it came from.
        let kind = columns.remove(2);
        columns.insert(0, kind);
        move_slot(&mut values, 2, 0);
        assert_eq!(
            pairs(&columns, &values),
            vec![("artist", "a"), ("genre", "g"), ("year", "y")]
        );

        // A column added before the first rebuild has no list yet.
        let mut unbuilt: Vec<&str> = Vec::new();
        remove_slot(&mut unbuilt, 2);
        move_slot(&mut unbuilt, 1, 0);
        assert!(unbuilt.is_empty());
    }

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
