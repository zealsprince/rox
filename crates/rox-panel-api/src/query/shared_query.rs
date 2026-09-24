//! The app-wide search query and the per-view knob that opts a panel into
//! following it. One [`SharedQuery`] entity on the app state holds the text
//! and the filter panel's picks. A global-following panel publishes its box's
//! text here and every follower reads it back, so one box drives them all.
//! [`QuerySource`] is the panel-config knob, and [`QueryFilter`] gives a
//! searching panel the follow-and-mirror behavior.

use gpui::{
    AnyElement, App, Context, Div, Entity, EntityId, EventEmitter, SharedString, Window, div,
    prelude::*, px, svg,
};
use gpui_component::Side;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_library::projection::{FilterField, FilterSet};
use serde::{Deserialize, Serialize};

use crate::panel;
use crate::query::search::SearchBox;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_services::selection::Selection;

pub enum SharedQueryEvent {
    Changed,
}

#[derive(Default)]
pub struct SharedQuery {
    text: String,
    filter: FilterSet,
    /// Live search panels. A follower's jump opens its own box only when
    /// there's none.
    boxes: usize,
}

impl EventEmitter<SharedQueryEvent> for SharedQuery {}

impl SharedQuery {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn register_box(&mut self) {
        self.boxes += 1;
    }

    pub fn release_box(&mut self) {
        self.boxes = self.boxes.saturating_sub(1);
    }

    pub fn has_box(&self) -> bool {
        self.boxes > 0
    }

    /// A no-op when unchanged, which stops the echo: a follower's box copies
    /// the value back and publishes it again.
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

    /// Fires the same Changed as a text edit, behind the same echo guard.
    pub fn set_filter(&mut self, filter: FilterSet, cx: &mut Context<Self>) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        cx.emit(SharedQueryEvent::Changed);
        cx.notify();
    }
}

fn field_label(field: FilterField) -> gpui::SharedString {
    match field {
        FilterField::Artist => rox_i18n::t!("filter-field-artist"),
        FilterField::AlbumArtist => rox_i18n::t!("filter-field-album-artist"),
        FilterField::Album => rox_i18n::t!("filter-field-album"),
        FilterField::Genre => rox_i18n::t!("filter-field-genre"),
        FilterField::Year => rox_i18n::t!("filter-field-year"),
        FilterField::Folder => rox_i18n::t!("filter-field-folder"),
        FilterField::Source => rox_i18n::t!("filter-field-source"),
    }
}

fn value_label(field: FilterField, value: &str) -> String {
    match field {
        FilterField::Year if value == "0" => rox_i18n::t!("filter-unknown").to_string(),
        // A pick holds the stored source string, a digest for a server.
        FilterField::Source => rox_library::cue::source_label(value),
        _ if value.is_empty() => rox_i18n::t!("filter-unknown").to_string(),
        _ => value.to_string(),
    }
}

/// Quoted so a value with spaces stays one term.
pub fn field_term(field: &str, value: &str) -> String {
    format!("{field}:\"{value}\"")
}

/// Toggle one exact value on the shared filter, the filter panel's own path.
pub fn toggle_pick(query: &Entity<SharedQuery>, field: FilterField, value: &str, cx: &mut App) {
    query.update(cx, |query, cx| {
        let mut filter = query.filter().clone();
        filter.toggle(field, value);
        query.set_filter(filter, cx);
    });
}

/// For fields the filter has no column for (the title): appends the term to
/// what's typed, or takes it back off.
pub fn toggle_term(query: &Entity<SharedQuery>, field: &str, value: &str, cx: &mut App) {
    query.update(cx, |query, cx| {
        let next = toggled_term(query.text(), &field_term(field, value));
        query.set(next, cx);
    });
}

/// The quoted `field:"value"` form is distinctive enough that a plain find
/// is the whole match.
fn toggled_term(text: &str, term: &str) -> String {
    let Some(at) = text.find(term) else {
        return match text.trim_end() {
            "" => term.to_string(),
            head => format!("{head} {term}"),
        };
    };
    let head = text[..at].trim_end();
    let tail = text[at + term.len()..].trim_start();
    match (head.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_string(),
        (false, true) => head.to_string(),
        (false, false) => format!("{head} {tail}"),
    }
}

/// One removable chip per picked value, then a Clear. None on an empty
/// filter, so the host only spends the space when there's one.
pub fn filter_chips(query: &Entity<SharedQuery>, cx: &App) -> Option<Div> {
    let filter = query.read(cx).filter().clone();
    // Field picks only: an id pin has no chip, and a lone Clear would control
    // nothing.
    if filter.fields_empty() {
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
            .child(rox_i18n::t!("filter-clear")),
    );
    Some(strip)
}

/// Shared by default, so the search panel filters a fresh layout with no
/// per-panel setup. Selection shows the tracks another panel last picked and
/// keeps its own box like [`QuerySource::Local`], so text narrows within it.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuerySource {
    Local,
    #[default]
    Global,
    Selection,
}

fn source_items<P: 'static>(
    mut menu: PopupMenu,
    get: impl Fn(&P) -> QuerySource + Clone + 'static,
    is_shown: impl Fn(&P) -> bool + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    toggle: impl Fn(&mut P, bool, &mut Context<P>) + Clone + 'static,
) -> PopupMenu {
    let shown_read = is_shown.clone();
    menu = menu.item(panel::check_row(
        rox_i18n::t!("query-show-search-box"),
        Some(icons::EYE),
        is_shown,
        move |this, cx| {
            let on = shown_read(this);
            toggle(this, !on, cx);
        },
        panel,
    ));
    for (label, icon, source) in [
        (
            rox_i18n::t!("query-own-query"),
            icons::SEARCH,
            QuerySource::Local,
        ),
        (
            rox_i18n::t!("query-shared-query"),
            icons::GLOBE,
            QuerySource::Global,
        ),
        (
            rox_i18n::t!("query-source-selection"),
            icons::PIN,
            QuerySource::Selection,
        ),
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

/// The query-source knob as a "Search" flyout on a panel's Display menu.
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
        // Follow the panel so the ticks swap live while the flyout is open.
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
    menu.item(PopupMenuItem::submenu(
        rox_i18n::t!("query-search"),
        submenu,
    ))
}

/// The shared "search" section for a searching panel's Behavior page, so
/// every searching panel reads the same.
pub fn search_section<P: 'static>(
    show: bool,
    on_show: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    source: QuerySource,
    on_source: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> AnyElement {
    rox_panel_kit::ui::section(
        rox_i18n::t!("query-search"),
        None,
        div()
            .flex()
            .flex_col()
            .gap(rox_design::tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("query-search-box"),
                Some(rox_i18n::t!("query-search-box.description")),
                panel::toggle(show, on_show, cx),
            ))
            .child(source_row(source, on_source, cx)),
    )
    .into_any_element()
}

pub fn source_row<P: 'static>(
    current: QuerySource,
    on_pick: impl Fn(&mut P, QuerySource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::setting_row(
        rox_i18n::t!("query-source"),
        Some(rox_i18n::t!("query-source.description")),
        panel::choices_shared(
            &[
                (rox_i18n::t!("query-source-shared"), QuerySource::Global),
                (rox_i18n::t!("query-source-own"), QuerySource::Local),
                (
                    rox_i18n::t!("query-source-selection"),
                    QuerySource::Selection,
                ),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}

/// A searching panel's shared query behavior. The panel wires the accessors
/// to its fields and still owns the plumbing: route the shared query
/// subscription to [`QueryFilter::on_shared_query_changed`], the box's
/// `Changed` to [`QueryFilter::on_query_box_changed`], and call
/// [`QueryFilter::sync_query_box`] in render.
pub trait QueryFilter: Sized + 'static {
    fn shared_query(&self) -> &Entity<SharedQuery>;
    fn selection(&self) -> &Entity<Selection>;
    /// The pinned ids while following the selection. Held rather than read
    /// live, because the panel's own picks must not move them.
    fn selection_ids(&self) -> &[i64];
    fn set_selection_ids(&mut self, ids: Vec<i64>);
    fn query_box(&self) -> &Entity<SearchBox>;
    fn query_source(&self) -> QuerySource;
    fn set_query_source_value(&mut self, source: QuerySource);
    /// Kept while following the shared query, so switching back has something
    /// to restore.
    fn local_query(&self) -> String;
    fn set_local_query(&mut self, query: String);
    /// Gates the own query only. A panel can follow the shared one with no box.
    fn query_box_shown(&self) -> bool;
    fn set_query_box_shown(&mut self, shown: bool);
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>);
    /// Consumed in render by [`QueryFilter::sync_query_box`].
    fn set_query_resync(&mut self, pending: bool);
    /// For the tab-title repaint most panels need.
    fn after_query_change(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
    }

    fn effective_query(&self, cx: &App) -> String {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection if self.query_box_shown() => {
                self.local_query()
            }
            QuerySource::Local | QuerySource::Selection => String::new(),
        }
    }

    /// An own-query panel ignores the shared picks, since the filter is a
    /// shared-search surface.
    fn effective_filter(&self, cx: &App) -> FilterSet {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).filter().clone(),
            QuerySource::Local => FilterSet::default(),
            QuerySource::Selection => FilterSet::with_ids(self.selection_ids().to_vec()),
        }
    }

    /// Independent of the box's visibility, so a hidden own-query box keeps
    /// its text.
    fn query_box_text(&self, cx: &App) -> String {
        match self.query_source() {
            QuerySource::Global => self.shared_query().read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => self.local_query(),
        }
    }

    /// Guarded on drift so a box being typed in keeps its cursor, which also
    /// stops the sync echo. Call from render.
    fn sync_query_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.query_box_text(cx);
        self.query_box().clone().update(cx, |box_, cx| {
            if box_.query() != text {
                box_.set_value(&text, window, cx);
            }
        });
    }

    /// The box resets to the new source on the next render.
    fn pick_query_source(&mut self, source: QuerySource, cx: &mut Context<Self>) {
        if self.query_source() == source {
            return;
        }
        self.set_query_source_value(source);
        // Pick up the current selection rather than an empty view until the
        // next pick.
        if source == QuerySource::Selection {
            let ids = self.selection().read(cx).tracks().to_vec();
            self.set_selection_ids(ids);
        }
        self.set_query_resync(true);
        self.rebuild_query_view(cx);
        cx.notify();
        self.after_query_change(cx);
    }

    fn on_query_box_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.query_box().read(cx).query().to_string();
        match self.query_source() {
            QuerySource::Global => {
                self.shared_query()
                    .clone()
                    .update(cx, |q, cx| q.set(text, cx));
            }
            QuerySource::Local | QuerySource::Selection => {
                self.set_local_query(text);
                self.rebuild_query_view(cx);
            }
        }
        cx.notify();
        self.after_query_change(cx);
    }

    /// Faceted browse: pin one field to an exact value on the active source.
    /// A shared-query follower leaves showing it to a search panel and opens
    /// its own box only when none is up. An own query only filters while its
    /// box shows.
    fn jump_to_query(&mut self, field: &str, value: &str, cx: &mut Context<Self>) {
        if value.is_empty() {
            return;
        }
        let query = field_term(field, value);
        match self.query_source() {
            QuerySource::Global => {
                if !self.shared_query().read(cx).has_box() {
                    self.set_query_box_shown(true);
                }
                self.shared_query()
                    .clone()
                    .update(cx, |q, cx| q.set(query, cx));
            }
            // The id pin still holds, so this narrows within the selection.
            QuerySource::Local | QuerySource::Selection => {
                self.set_query_box_shown(true);
                self.set_local_query(query);
                self.rebuild_query_view(cx);
            }
        }
        self.set_query_resync(true);
        cx.notify();
        self.after_query_change(cx);
    }

    /// A panel's own picks are ignored: a selection-following view still
    /// publishes its clicks to keep the chain running, and re-pinning to them
    /// would narrow it to one row with no way back.
    fn on_selection_changed(&mut self, source: EntityId, cx: &mut Context<Self>) {
        if self.query_source() != QuerySource::Selection || source == cx.entity().entity_id() {
            return;
        }
        let ids = self.selection().read(cx).tracks().to_vec();
        self.set_selection_ids(ids);
        self.rebuild_query_view(cx);
        cx.notify();
        self.after_query_change(cx);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_term_lands_on_the_end_of_whats_typed() {
        assert_eq!(toggled_term("", "title:\"Dockside\""), "title:\"Dockside\"");
        assert_eq!(
            toggled_term("remix", "title:\"Dockside\""),
            "remix title:\"Dockside\""
        );
    }

    #[test]
    fn the_same_term_again_comes_back_off() {
        assert_eq!(toggled_term("title:\"Dockside\"", "title:\"Dockside\""), "");
        assert_eq!(
            toggled_term("remix title:\"Dockside\"", "title:\"Dockside\""),
            "remix"
        );
        assert_eq!(
            toggled_term("remix title:\"Dockside\" 2013", "title:\"Dockside\""),
            "remix 2013"
        );
        assert_eq!(
            toggled_term("title:\"Dockside\" remix", "title:\"Dockside\""),
            "remix"
        );
    }
}
