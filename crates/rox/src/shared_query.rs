//! The app-wide search query and the per-view knob that opts a panel into
//! following it. The query lives in one [`SharedQuery`] entity on the app
//! state, shared the way [`crate::selection::Selection`] is: a
//! global-following panel publishes its box's text here and every other
//! follower reads it back, so one box drives them all. [`QuerySource`] is the
//! panel-config knob, own or shared, and [`QueryFilter`] is the trait a
//! searching panel implements to get the whole follow-and-mirror behavior for
//! free - the library, the grids, and the art shelf all ride it. Shared is the
//! default, so a fresh panel follows the search box out of the box; a panel
//! set to its own query keeps an independent filter, the duplicate-with-config
//! story.

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Div, Entity, EventEmitter, SharedString,
    Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Side;
use rox_library::projection::{FilterField, FilterSet};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel;
use crate::search::SearchBox;

/// The shared query changed; global-following panels re-read it.
pub enum SharedQueryEvent {
    Changed,
}

/// The one app-wide search query. Global-following panels write it from
/// their box and read it back to filter. The structured filter rides
/// along: the filter panel writes its exact-value picks here, and the
/// same followers narrow by both.
#[derive(Default)]
pub struct SharedQuery {
    text: String,
    /// The filter panel's exact-value picks, applied alongside the text
    /// by every global-following panel.
    filter: FilterSet,
    /// How many search panels are alive to show this query. A jump-to action
    /// from a follower checks this to decide whether the filter already has a
    /// box to land in, or whether it has to open the follower's own.
    boxes: usize,
}

impl EventEmitter<SharedQueryEvent> for SharedQuery {}

impl SharedQuery {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// A search panel came up; count it while it lives.
    pub fn register_box(&mut self) {
        self.boxes += 1;
    }

    /// A search panel went away.
    pub fn release_box(&mut self) {
        self.boxes = self.boxes.saturating_sub(1);
    }

    /// Whether a search panel is up somewhere to show the shared query.
    pub fn has_box(&self) -> bool {
        self.boxes > 0
    }

    /// Set the query. A no-op when unchanged, which is what stops the echo:
    /// a follower mirrors the new value back into its own box, that fires a
    /// change, and the change publishes the same text right back here, where
    /// it lands equal and goes no further.
    pub fn set(&mut self, text: String, cx: &mut Context<Self>) {
        if self.text == text {
            return;
        }
        self.text = text;
        cx.emit(SharedQueryEvent::Changed);
        cx.notify();
    }

    pub fn filter(&self) -> &FilterSet {
        &self.filter
    }

    /// Swap the structured filter. Fires the same Changed as a text edit,
    /// so every follower re-filters through the subscription it already
    /// holds; the same equality guard stops the echo.
    pub fn set_filter(&mut self, filter: FilterSet, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        cx.emit(SharedQueryEvent::Changed);
        cx.notify();
    }
}

/// The shared filter's field name, for a chip's `Field: Value` label.
fn field_label(field: FilterField) -> &'static str {
    match field {
        FilterField::Artist => "Artist",
        FilterField::AlbumArtist => "Album Artist",
        FilterField::Album => "Album",
        FilterField::Genre => "Genre",
        FilterField::Year => "Year",
    }
}

/// A picked value's display text: untagged reads as Unknown, the same way
/// the filter panel shows it. Year zero is the untagged marker.
fn value_label(field: FilterField, value: &str) -> String {
    match field {
        FilterField::Year if value == "0" => "Unknown".to_string(),
        _ if value.is_empty() => "Unknown".to_string(),
        _ => value.to_string(),
    }
}

/// The active-filter chips: one removable chip per picked value in the
/// shared filter, then a Clear that drops the lot. Returns None on an empty
/// filter so a host only spends the space when there's a filter to show.
/// Clicking a chip drops just its value, Clear drops the whole filter; the
/// text query is left alone either way. Every write lands on the shared
/// filter, which every follower already watches, so the views narrow and
/// widen with the chips.
pub fn filter_chips(query: &Entity<SharedQuery>, cx: &App) -> Option<Div> {
    let filter = query.read(cx).filter().clone();
    if filter.is_empty() {
        return None;
    }
    let mut strip = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(tokens::SPACE_XS);
    let mut ix = 0usize;
    for (field, values) in &filter.fields {
        for value in values {
            let q = query.clone();
            let (field, value) = (*field, value.clone());
            let label = format!("{}: {}", field_label(field), value_label(field, &value));
            strip = strip.child(
                div()
                    .id(("filter-chip", ix))
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .pl(tokens::SPACE_XS)
                    .pr(px(3.))
                    .py(px(1.))
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_control())
                    .text_xs()
                    .text_color(palette::text())
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .on_click(move |_, _, cx| {
                        q.update(cx, |query, cx| {
                            let mut filter = query.filter().clone();
                            filter.toggle(field, &value);
                            query.set_filter(filter, cx);
                        });
                    })
                    .child(SharedString::from(label))
                    .child(
                        svg()
                            .path(icons::CLOSE)
                            .size(px(10.))
                            .text_color(palette::text_muted()),
                    ),
            );
            ix += 1;
        }
    }
    let q = query.clone();
    strip = strip.child(
        div()
            .id("filter-chips-clear")
            .flex()
            .items_center()
            .px(tokens::SPACE_XS)
            .py(px(1.))
            .rounded(tokens::RADIUS)
            .text_xs()
            .text_color(palette::text_muted())
            .cursor_pointer()
            .hover(|d| d.text_color(palette::text()))
            .on_click(move |_, _, cx| {
                q.update(cx, |query, cx| query.set_filter(FilterSet::default(), cx));
            })
            .child("Clear"),
    );
    Some(strip)
}

/// Where a searching panel's query comes from: its own box, or the shared
/// app-wide one. Shared by default, so the search panel filters a fresh
/// layout with no per-panel setup.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuerySource {
    Local,
    #[default]
    Global,
}

/// The query-source rows for a panel's dropdown menu: the box toggle over
/// one checked entry per source, the same knob as [`source_row`]. Shared by
/// [`search_flyout`] so the follow mode reads the same everywhere. The check
/// sits on the right so each source keeps its icon.
fn source_items<P: 'static>(
    mut menu: PopupMenu,
    get: impl Fn(&P) -> QuerySource + Clone + 'static,
    is_shown: impl Fn(&P) -> bool + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    toggle: impl Fn(&mut P, bool, &mut Context<P>) + Clone + 'static,
) -> PopupMenu {
    // Show or hide the panel's own box first; the source rows below only
    // matter once there's a box to type in.
    let shown_read = is_shown.clone();
    menu = menu.item(panel::check_row(
        "Show Search Box",
        Some(icons::EYE),
        is_shown,
        move |this, cx| {
            let on = shown_read(this);
            toggle(this, !on, cx);
        },
        panel,
    ));
    for (label, icon, source) in [
        ("Own Query", icons::SEARCH, QuerySource::Local),
        ("Shared Query", icons::GLOBE, QuerySource::Global),
    ] {
        let get = get.clone();
        let set = set.clone();
        menu = menu.item(panel::check_row(
            label,
            Some(icon),
            move |this: &P| get(this) == source,
            move |this, cx| set(this, source, cx),
            panel,
        ));
    }
    menu
}

/// The query-source knob as a "Search" flyout on a panel's Display menu, so
/// the box toggle and source rows only show on hover.
#[allow(clippy::too_many_arguments)]
pub fn search_flyout<P: 'static>(
    menu: PopupMenu,
    get: impl Fn(&P) -> QuerySource + Clone + 'static,
    is_shown: impl Fn(&P) -> bool + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    toggle: impl Fn(&mut P, bool, &mut Context<P>) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let panel = panel.clone();
    let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
        // Follow the panel so the box toggle and the picked source tick swap
        // live while the flyout stays open.
        panel::follow_panel(&panel, cx);
        source_items(
            submenu.check_side(Side::Right),
            get,
            is_shown,
            &panel,
            set,
            toggle,
        )
    });
    menu.item(PopupMenuItem::submenu("Search", submenu))
}

/// The shared "search" section for a searching panel's Behavior page: the
/// box toggle over the source picker, one header so the library, the grids,
/// and the art shelf all read the same. The show toggle carries the panel's
/// own side effects (rebuild, retitle); the source pick routes to
/// [`pick_query_source`](QueryFilter) the same way everywhere.
pub fn search_section<P: 'static>(
    show: bool,
    on_show: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    source: QuerySource,
    on_source: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> AnyElement {
    crate::settings_ui::section(
        "Search",
        None,
        div()
            .flex()
            .flex_col()
            .gap(crate::design::tokens::SPACE_MD)
            .child(panel::setting_row(
                "Search Box",
                Some("Show the search box; the query only applies while it shows"),
                panel::toggle(show, on_show, cx),
            ))
            .child(source_row(source, on_source, cx)),
    )
    .into_any_element()
}

/// The query-source setting row for a searching panel's customize window.
pub fn source_row<P: 'static>(
    current: QuerySource,
    on_pick: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::setting_row(
        "Search Source",
        Some("Follow the shared search query, or filter by this panel's own box"),
        panel::choices(
            &[("Shared", QuerySource::Global), ("Own", QuerySource::Local)],
            current,
            on_pick,
            cx,
        ),
    )
}

/// A searching panel's shared query behavior, so the library, the grids, and
/// the art shelf don't each hand-roll the follow-and-mirror logic. A panel
/// wires the accessors to its own fields; the provided methods do the rest -
/// resolve the effective query, mirror the box, publish edits, and react to a
/// shared-query change. The panel still owns the plumbing: subscribe to the
/// shared query and route it to [`QueryFilter::on_shared_query_changed`],
/// route the box's `Changed` to [`QueryFilter::on_query_box_changed`], and
/// clear its resync flag in render by calling [`QueryFilter::sync_query_box`].
pub trait QueryFilter: Sized + 'static {
    /// The shared query entity, from the panel's `AppState`.
    fn shared_query(&self) -> &Entity<SharedQuery>;
    /// The panel's search box.
    fn query_box(&self) -> &Entity<SearchBox>;
    fn query_source(&self) -> QuerySource;
    fn set_query_source_value(&mut self, source: QuerySource);
    /// The panel's own private query, kept while following the shared one so
    /// the switch back to own has something to restore.
    fn local_query(&self) -> String;
    fn set_local_query(&mut self, query: String);
    /// Whether the panel's own box shows, gating its own query. The shared
    /// query ignores this: a panel can follow it with no box of its own.
    fn query_box_shown(&self) -> bool;
    /// Show or hide the panel's own box. A faceted jump opens it so the pinned
    /// filter is visible and clearable.
    fn set_query_box_shown(&mut self, shown: bool);
    /// Rebuild the view for the current effective query (the panel's own
    /// `refresh`/`rebuild`).
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>);
    /// Store the pending-box-reset flag, consumed in render by
    /// [`QueryFilter::sync_query_box`].
    fn set_query_resync(&mut self, pending: bool);
    /// A hook after any query change, for the tab-title repaint most panels
    /// need. Default does nothing.
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
    }

    /// The query the view filters by: the shared query while following it,
    /// the box's own text while its box shows, nothing while hidden.
    fn effective_query(&self, cx: &App) -> String {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).text().to_string(),
            QuerySource::Local if self.query_box_shown() => self.local_query(),
            QuerySource::Local => String::new(),
        }
    }

    /// The structured filter the view narrows by, alongside the effective
    /// query: the shared picks while following the shared query, nothing on
    /// a panel's own - the filter is a shared-search surface, so an
    /// own-query panel stays out of its reach.
    fn effective_filter(&self, cx: &App) -> FilterSet {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).filter().clone(),
            QuerySource::Local => FilterSet::default(),
        }
    }

    /// The text the box should show for the active source: the shared query
    /// while following, the panel's own otherwise. Independent of the box's
    /// own visibility, so a hidden own-query box keeps its text.
    fn query_box_text(&self, cx: &App) -> String {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).text().to_string(),
            QuerySource::Local => self.local_query(),
        }
    }

    /// Reset the box to the active source's text, cursor to the end. Guarded
    /// on drift so the box the user is typing in keeps its cursor, which also
    /// stops the mirror echo. Call from render, where a window exists.
    fn sync_query_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.query_box_text(cx);
        self.query_box().clone().update(cx, |box_, cx| {
            if box_.query() != text {
                box_.set_value(&text, window, cx);
            }
        });
    }

    /// Switch this panel between its own query and the shared one. The box
    /// resets to the newly active source on the next render.
    fn pick_query_source(&mut self, source: QuerySource, cx: &mut Context<Self>) {
        if self.query_source() == source {
            return;
        }
        self.set_query_source_value(source);
        self.set_query_resync(true);
        self.rebuild_query_view(cx);
        cx.notify();
        self.after_query_change(cx);
    }

    /// Route the box's `Changed` here: publish to the shared query while
    /// following it, otherwise filter locally. `local_query` stays the
    /// panel's own, preserved for the switch back.
    fn on_query_box_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.query_box().read(cx).query().to_string();
        match self.query_source() {
            QuerySource::Global => {
                self.shared_query()
                    .clone()
                    .update(cx, |q, cx| q.set(text, cx));
            }
            QuerySource::Local => {
                self.set_local_query(text);
                self.rebuild_query_view(cx);
            }
        }
        cx.notify();
        self.after_query_change(cx);
    }

    /// Drive the panel's search to one field's exact value, the cheap faceted
    /// browse: right-click a row or tile, jump to its album or artist. Writes
    /// the query on whichever source is active and rebuilds; the value is quoted
    /// so spaces stay one term. While following the shared query the filter
    /// applies with no box of our own, so let a search panel show it and only
    /// open our box as the fallback when none is up. An own query only filters
    /// while its box shows, so that path always opens it.
    fn jump_to_query(&mut self, field: &str, value: &str, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        let query = format!("{field}:\"{value}\"");
        match self.query_source() {
            QuerySource::Global => {
                if !self.shared_query().read(cx).has_box() {
                    self.set_query_box_shown(true);
                }
                self.shared_query()
                    .clone()
                    .update(cx, |q, cx| q.set(query, cx));
            }
            QuerySource::Local => {
                self.set_query_box_shown(true);
                self.set_local_query(query);
                self.rebuild_query_view(cx);
            }
        }
        self.set_query_resync(true);
        cx.notify();
        self.after_query_change(cx);
    }

    /// Route the shared-query subscription here: re-filter and reset the box
    /// while following, ignore it otherwise.
    fn on_shared_query_changed(&mut self, cx: &mut Context<Self>) {
        if self.query_source() != QuerySource::Global {
            return;
        }
        self.set_query_resync(true);
        self.rebuild_query_view(cx);
        cx.notify();
        self.after_query_change(cx);
    }
}
