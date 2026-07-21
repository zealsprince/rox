//! The filter panel: the library's field values as cascading columns,
//! each one filter field - artist, album artist, album, genre, or year -
//! listing every distinct value with its track count. Picking values
//! writes the shared query's structured filter, so every global-following
//! panel narrows with it, and each column here narrows by the picks in
//! the columns left of it, the column-browser cascade. The shared text
//! query narrows the value lists too, so the panel and the search boxes
//! read the same library. Columns are per-panel config; the picks are the
//! one app-wide filter, so two filter panels share them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, svg, uniform_list, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::projection::{FilterField, FilterSet, Projection, SymTable};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::query::shared_query::SharedQueryEvent;

/// One value row's height; the lists are uniform_lists, so every row
/// agrees.
const ROW_H: f32 = 26.;

/// How long a type-ahead phrase keeps growing before the next keystroke
/// starts a fresh jump.
const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// A column's filter field, the per-panel half of the story; the picks
/// themselves live on the shared query.
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
            ColumnKind::Artist => "Artist",
            ColumnKind::AlbumArtist => "Album Artist",
            ColumnKind::Album => "Album",
            ColumnKind::Genre => "Genre",
            ColumnKind::Year => "Year",
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
/// how many context tracks carry it, and whether it is picked.
struct Value {
    label: SharedString,
    value: String,
    count: u32,
    selected: bool,
}

/// A header drag in flight: the column it started from, so a drop on
/// another header knows what to move. The label rides along for the
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
    /// query, or the picks change - never per frame.
    columns: Vec<Vec<Value>>,
    scrolls: Vec<UniformListScrollHandle>,
    /// The column the keyboard drives: type-ahead and arrows move within
    /// it, and the cursor highlight lives in it. Set by clicking a value or
    /// stepping left and right.
    active_col: usize,
    /// The keyboard cursor, a row index in the active column: where arrows
    /// move from and enter toggles. None until a key or click lands one.
    cursor: Option<usize>,
    /// The type-ahead phrase and when its last keystroke landed, so a quick
    /// run of letters jumps to a value by prefix.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _query_changed: Subscription,
}

impl FilterPanel {
    pub fn new(
        state: AppState,
        config: FilterConfig,
        _window: &mut Window,
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
        // The picks land here too: a toggle writes the shared filter, the
        // Changed comes back around, and the cascade rebuilds once.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| this.refresh(cx),
        );
        let mut this = FilterPanel {
            state,
            config,
            columns: Vec::new(),
            scrolls: Vec::new(),
            active_col: 0,
            cursor: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _query_changed,
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
                if self.type_ahead.is_empty() && text == " " {
                    return;
                }
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
        let Some(values) = self.columns.get(self.active_col) else {
            return;
        };
        let needle = self.type_ahead.to_lowercase();
        // A grown phrase re-tests the current row; a fresh one starts past
        // it, so the same first letter walks to the next match.
        let start = match self.cursor {
            Some(ix) if grown => ix,
            Some(ix) => ix + 1,
            None => 0,
        };
        let len = values.len();
        let hit = (0..len)
            .map(|off| (start + off) % len)
            .find(|&ix| values[ix].label.to_lowercase().starts_with(&needle));
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
    /// cursor lands on the edge it heads toward.
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

    /// Rebuild every column's values. The context starts as the shared
    /// text query's hits and narrows left to right: each column lists the
    /// values in its context, then its own picks cut the context for the
    /// columns after it - so a picked artist leaves the artist list whole
    /// but narrows the albums.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (text, filter) = {
            let query = self.state.query.read(cx);
            (query.text().to_string(), query.filter().clone())
        };
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            self.columns = self.config.columns.iter().map(|_| Vec::new()).collect();
            self.scrolls
                .resize_with(self.config.columns.len(), UniformListScrollHandle::new);
            self.clamp_cursor();
            cx.notify();
            return;
        };
        let mut rows: Vec<u32> = if text.is_empty() {
            (0..projection.len() as u32).collect()
        } else {
            projection.search(&text)
        };
        self.columns = self
            .config
            .columns
            .iter()
            .map(|&kind| {
                let picks = filter.values(kind.field());
                let values = column_values(projection, kind, &rows, picks);
                if !picks.is_empty() {
                    let mut sub = FilterSet::default();
                    sub.fields.push((kind.field(), picks.to_vec()));
                    if let Some(mask) = projection.filter_mask(&sub) {
                        rows.retain(|&row| mask[row as usize]);
                    }
                }
                values
            })
            .collect();
        self.scrolls
            .resize_with(self.columns.len(), UniformListScrollHandle::new);
        self.clamp_cursor();
        cx.notify();
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
        if self.config.columns.contains(&kind) {
            self.config.columns.retain(|&k| k != kind);
            self.drop_picks_if_unused(kind, cx);
        } else {
            self.config.columns.push(kind);
        }
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
        self.drop_picks_if_unused(old, cx);
        self.refresh(cx);
    }

    fn remove_column(&mut self, col: usize, cx: &mut Context<Self>) {
        if col >= self.config.columns.len() {
            return;
        }
        let old = self.config.columns.remove(col);
        self.drop_picks_if_unused(old, cx);
        self.refresh(cx);
    }

    /// Append a column of the field: the + button's and empty state's add.
    /// Twins are allowed, same as a header's kind pick, so the + can stack
    /// a second Album column if you want one.
    fn add_column(&mut self, kind: ColumnKind, cx: &mut Context<Self>) {
        self.config.columns.push(kind);
        self.refresh(cx);
    }

    /// Move a column to another slot, the header drag's landing. Picks ride
    /// along untouched since every field keeps its column.
    fn move_column(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.config.columns.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let kind = self.config.columns.remove(from);
        // Removing from ahead of the target shifts the target back one.
        let dest = if from < to { to - 1 } else { to };
        self.config.columns.insert(dest, kind);
        self.refresh(cx);
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
    /// lands here.
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
                                PopupMenuItem::new("Clear Selection")
                                    .icon(Icon::default().path(icons::CLOSE))
                                    .disabled(!picked)
                                    .on_click(move |_, _, cx| {
                                        let Some(this) = clear.upgrade() else { return };
                                        this.update(cx, |this, cx| this.clear_column(col, cx));
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Remove Column")
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
            button.label("Add Column").outline()
        } else {
            button.ghost().tooltip("Add column")
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
            .h(px(ROW_H))
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
            .child(div().flex_1().min_w_0().truncate().child("All"))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(distinct.to_string())),
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
                        .h(px(ROW_H))
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
                        // "where typing landed" without stealing the picked
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
                                .child(SharedString::from(value.count.to_string())),
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
    /// leaf click would dead-end there - the tab-owned root never hears
    /// the dismiss and the menu hangs open with its checks frozen. Flat
    /// rows dismiss the root cleanly, so the next open reads the change. A
    /// twin column made through a header's kind pick still counts as
    /// shown.
    fn columns_menu(&self, mut menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let shown = self.config.columns.clone();
        menu = menu.label("Columns");
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Filter")
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

    /// The layout dump carries the panel's config; the builder registered
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
            PopupMenuItem::new("Clear Filters")
                .icon(Icon::default().path(icons::CLOSE))
                .disabled(!filtering)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.clear_all(cx));
                }),
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the column layout along, like the history's.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| FilterPanel::new(state, config, window, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.on_panel_key(event, cx)
            }));
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
                            .child("Pick a field to start filtering"),
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
                        uniform_list(("filter-values", col), count, move |range, _, cx| {
                            this.upgrade()
                                .map(|this| {
                                    this.update(cx, |this, cx| this.list_rows(col, range, cx))
                                })
                                .unwrap_or_default()
                        })
                        .track_scroll(self.scrolls[col].clone())
                        .flex_1()
                        .w_full(),
                    ),
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
        root.child(cols)
    }
}

/// One column's value rows out of its context: every distinct value with
/// its track count, alphabetical for the interned fields, ascending for
/// years. A pick whose value fell out of the context (the text query
/// moved on) stays listed at zero so it can still be cleared.
fn column_values(
    projection: &Projection,
    kind: ColumnKind,
    rows: &[u32],
    picks: &[String],
) -> Vec<Value> {
    let mut out = match kind {
        ColumnKind::Year => {
            let mut counts: HashMap<u16, u32> = HashMap::new();
            for &row in rows {
                *counts.entry(projection.year[row as usize]).or_default() += 1;
            }
            let mut years: Vec<(u16, u32)> = counts.into_iter().collect();
            years.sort_unstable_by_key(|&(year, _)| year);
            years
                .into_iter()
                .map(|(year, count)| {
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
        _ => {
            let (column, table) = sym_source(projection, kind);
            let mut counts = vec![0u32; table.strings.len()];
            for &row in rows {
                counts[column[row as usize] as usize] += 1;
            }
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
    out
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
        "Unknown".into()
    } else {
        SharedString::from(value.to_string())
    }
}

/// Year zero is the untagged marker, the scanner's default.
fn year_label(year: u16) -> SharedString {
    if year == 0 {
        "Unknown".into()
    } else {
        SharedString::from(year.to_string())
    }
}
