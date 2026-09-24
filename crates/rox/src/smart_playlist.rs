//! The smart playlist editor: a window over one saved query, the definition on
//! the left and a live preview of what it takes on the right. The preview
//! re-evaluates on every change, one projection pass, never per frame.
//!
//! The structured filter passes through untouched: the filter panel builds its
//! controls, and dropping it would lose work the query text can't express.

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, Pixels, SharedString, Subscription, UniformListScrollHandle, Window, actions,
    div, prelude::*, px, size, uniform_list,
};
use gpui_component::Sizable;
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::playlists::SmartDef;
use rox_library::projection::{QUERY_FIELDS, SortKey};
use rox_panel_api::panel::AppState;
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::suggest;
use rox_panel_kit::ui::{self as settings_ui, Seg, checkbox, kbd_line, section, small_button};
use rox_services::backdrop::WindowBackdrop;

/// Fixed, so every pixel the window grows by goes to the preview.
const CONTROLS_W: Pixels = px(340.);

/// Also the indent that lines a note up under the control.
const LABEL_W: Pixels = px(64.);

const ROW_H: Pixels = px(22.);

/// A handful of fields people build lists around, not every table column.
/// Rebuilt per call since `t!` isn't const.
fn sorts() -> Vec<(SharedString, Option<SortKey>)> {
    vec![
        (rox_i18n::t!("smart-playlist-sort-default"), None),
        (rox_i18n::t!("info-item-title"), Some(SortKey::Title)),
        (rox_i18n::t!("head-piece-artist"), Some(SortKey::Artist)),
        (rox_i18n::t!("head-piece-album"), Some(SortKey::Album)),
        (rox_i18n::t!("head-piece-genre"), Some(SortKey::Genre)),
        (rox_i18n::t!("head-piece-year"), Some(SortKey::Year)),
        (rox_i18n::t!("info-item-duration"), Some(SortKey::Duration)),
        (rox_i18n::t!("info-item-rating"), Some(SortKey::Rating)),
        (rox_i18n::t!("status-item-plays"), Some(SortKey::Plays)),
        (
            rox_i18n::t!("smart-playlist-sort-added"),
            Some(SortKey::Added),
        ),
    ]
}

fn sort_label(sort: Option<SortKey>) -> SharedString {
    sorts()
        .into_iter()
        .find(|(_, key)| *key == sort)
        .map(|(label, _)| label)
        .unwrap_or_else(|| rox_i18n::t!("smart-playlist-sort-default"))
}

actions!(smart_playlist, [Save]);

const CONTEXT: &str = "SmartPlaylist";

/// Bound on the window root so Enter saves from anywhere. An open suggestion
/// menu swallows Enter first, so it takes the suggestion and saves on the next
/// press.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Save, Some(CONTEXT))]
}

/// The query never fails to parse: an unknown `foo:` prefix falls back to a
/// text term (see [`rox_library::projection::parse_query`]), which is worth
/// catching before saving a playlist that matches nothing.
fn query_note(query: &str) -> Option<SharedString> {
    let unknown = query
        .split_whitespace()
        // A quoted value arrives split across tokens; neither half names a
        // field.
        .filter(|token| !token.contains('"'))
        .find_map(|token| {
            let (name, _) = token.split_once(':')?;
            // A leading hyphen negates: `-genre:rock` names genre.
            let name = name.to_lowercase();
            let bare = name.strip_prefix('-').unwrap_or(&name);
            let known = QUERY_FIELDS.iter().any(|(field, _)| *field == bare);
            (!bare.is_empty() && !known).then(|| name.clone())
        })?;
    Some(rox_i18n::t!(
        "smart-playlist-unknown-field",
        field = unknown
    ))
}

pub fn open(state: AppState, id: Option<i64>, cx: &mut App) {
    let verb = if id.is_some() {
        rox_i18n::t!("smart-playlist-edit-title")
    } else {
        rox_i18n::t!("smart-playlist-new-title")
    };
    let title = rox_i18n::t!("smart-playlist-window-title", verb = verb.to_string());
    let bounds = Bounds::centered(None, size(px(900.), px(560.)), cx);
    rox_panel_api::panel::open_child_window(
        cx,
        title,
        bounds,
        // The definition column holds its width, so shrinking eats the preview
        // and stops before either is unusable.
        Some(settings_ui::MIN_SIZE),
        move |window, cx| cx.new(|cx| SmartPlaylistWindow::new(state, id, window, cx)),
    );
}

struct SmartPlaylistWindow {
    state: AppState,
    id: Option<i64>,
    name: Entity<InputState>,
    query: Entity<SearchBox>,
    limit: Entity<InputState>,
    sort: Option<SortKey>,
    descending: bool,
    /// Passed straight back through on save.
    filter: rox_library::projection::FilterSet,
    /// Rows rather than tracks: the preview resolves only the few it draws.
    matched: Vec<u32>,
    /// One Enter can reach [`Self::commit`] twice (the input's binding and the
    /// window's), and a second save would file a second playlist.
    saved: bool,
    scroll: UniformListScrollHandle,
    backdrop: WindowBackdrop,
    _query_events: Subscription,
    _name_events: Subscription,
    _limit_events: Subscription,
    _backdrop_changed: Subscription,
}

impl SmartPlaylistWindow {
    fn new(state: AppState, id: Option<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let existing = id.and_then(|id| state.library.read(cx).playlist_definition(id));
        let current_name = id
            .and_then(|id| {
                state
                    .library
                    .read(cx)
                    .playlists()
                    .into_iter()
                    .find(|playlist| playlist.id == id)
            })
            .map(|playlist| playlist.name)
            .unwrap_or_default();
        let def = existing.unwrap_or_default();

        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("smart-playlist-name-placeholder"))
                .default_value(current_name)
        });
        let query = cx.new(|cx| {
            SearchBox::new(
                rox_i18n::t!("smart-playlist-query-label"),
                &def.query,
                window,
                cx,
            )
            .small()
        });
        // The library box's completions, so the syntax is learnable here.
        let provider = suggest::query_provider(&state.library, cx);
        query.update(cx, |query, cx| query.set_completions(provider, cx));
        let limit = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("smart-playlist-limit-placeholder"))
                .default_value(def.limit.map(|n| n.to_string()).unwrap_or_default())
        });

        let _query_events = cx.subscribe_in(
            &query,
            window,
            |this: &mut Self, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Changed => this.requery(cx),
                SearchEvent::Submitted => this.commit(window, cx),
                _ => {}
            },
        );
        let _name_events = cx.subscribe_in(
            &name,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.commit(window, cx),
                _ => {}
            },
        );
        let _limit_events = cx.subscribe_in(
            &limit,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.requery(cx),
                InputEvent::PressEnter { .. } => this.commit(window, cx),
                _ => {}
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&name.read(cx).focus_handle(cx));

        let mut this = SmartPlaylistWindow {
            state,
            id,
            name,
            query,
            limit,
            sort: def.sort.map(|(key, _)| key),
            descending: def.sort.is_some_and(|(_, descending)| descending),
            filter: def.filter,
            matched: Vec::new(),
            saved: false,
            scroll: UniformListScrollHandle::new(),
            backdrop: WindowBackdrop::default(),
            _query_events,
            _name_events,
            _limit_events,
            _backdrop_changed,
        };
        this.requery(cx);
        this
    }

    fn definition(&self, cx: &App) -> SmartDef {
        let limit = self.limit.read(cx).value().trim().parse::<u32>().ok();
        SmartDef {
            query: self.query.read(cx).query().to_string(),
            filter: self.filter.clone(),
            sort: self.sort.map(|key| (key, self.descending)),
            // A zero cap reads as no cap.
            limit: limit.filter(|&n| n > 0),
        }
    }

    fn requery(&mut self, cx: &mut Context<Self>) {
        let def = self.definition(cx);
        self.matched = self.state.library.read(cx).smart_rows(&def);
        cx.notify();
    }

    fn set_sort(&mut self, sort: Option<SortKey>, cx: &mut Context<Self>) {
        self.sort = sort;
        self.requery(cx);
    }

    fn toggle_descending(&mut self, cx: &mut Context<Self>) {
        self.descending = !self.descending;
        self.requery(cx);
    }

    /// Only a blank name blocks the save; a query that takes nothing is a real
    /// thing to save.
    fn savable(&self, cx: &App) -> bool {
        !self.name.read(cx).value().trim().is_empty()
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if self.saved || name.is_empty() {
            return;
        }
        self.saved = true;
        let def = self.definition(cx);
        let id = self.id;
        self.state.library.update(cx, |library, cx| match id {
            Some(id) => {
                library.rename_playlist(id, &name, cx);
                library.set_playlist_definition(id, &def, cx);
            }
            None => {
                library.create_smart_playlist(&name, &def, cx);
            }
        });
        window.remove_window();
    }

    fn field(label: impl Into<SharedString>, control: impl IntoElement) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .w(LABEL_W)
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(label.into()),
            )
            .child(div().flex_1().min_w_0().child(control))
    }

    fn controls(&mut self, cx: &mut Context<Self>) -> Div {
        let weak = cx.entity().downgrade();
        let sort = self.sort;
        let descending = self.descending;
        let note = query_note(self.query.read(cx).query());
        let heading = if self.id.is_some() {
            rox_i18n::t!("smart-playlist-edit-title")
        } else {
            rox_i18n::t!("smart-playlist-new-title")
        };
        let fields = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(Self::field(
                rox_i18n::t!("panel-rename-name"),
                Input::new(&self.name).w_full(),
            ))
            .child(Self::field(
                rox_i18n::t!("smart-playlist-query-label"),
                self.query
                    .update(cx, |query, cx| query.element(cx))
                    .w_full(),
            ))
            .when_some(note, |d, note| {
                d.child(
                    div()
                        .pl(LABEL_W + tokens::SPACE_SM)
                        .text_xs()
                        .text_color(palette::tone_warn())
                        .child(note),
                )
            })
            .child(Self::field(
                rox_i18n::t!("smart-playlist-sort-label"),
                div()
                    .flex()
                    .flex_row()
                    // Wraps the direction onto its own line when a sort name
                    // runs long.
                    .flex_wrap()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        Button::new("smart-sort")
                            .label(sort_label(sort))
                            .small()
                            .outline()
                            .dropdown_menu(move |mut menu, _, _| {
                                for (label, key) in sorts() {
                                    let this = weak.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(label).checked(sort == key).on_click(
                                            move |_, _, cx| {
                                                if let Some(this) = this.upgrade() {
                                                    this.update(cx, |this, cx| {
                                                        this.set_sort(key, cx)
                                                    });
                                                }
                                            },
                                        ),
                                    );
                                }
                                menu
                            }),
                    )
                    .when(sort.is_some(), |d| {
                        d.child(
                            div()
                                .id("smart-descending")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(tokens::SPACE_XS)
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                        this.toggle_descending(cx)
                                    }),
                                )
                                .child(checkbox(descending))
                                .child(
                                    div()
                                        .text_color(palette::text_muted())
                                        .child(rox_i18n::t!("smart-playlist-descending")),
                                ),
                        )
                    }),
            ))
            .child(Self::field(
                rox_i18n::t!("smart-playlist-limit-label"),
                Input::new(&self.limit).w_full(),
            ));
        div()
            .w(CONTROLS_W)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .p(tokens::SPACE_MD)
            .child(section(heading, None, fields))
    }

    fn preview(&mut self, cx: &mut Context<Self>) -> Div {
        let count = rox_i18n::t!(
            "smart-playlist-match-count",
            count = self.matched.len() as u64
        );
        let this = cx.entity().downgrade();
        let list = if self.matched.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("smart-playlist-no-matches"))
                .into_any_element()
        } else {
            uniform_list("smart-matches", self.matched.len(), move |range, _, cx| {
                this.upgrade()
                    .map(|this| this.update(cx, |this, cx| this.preview_rows(range, cx)))
                    .unwrap_or_default()
            })
            .track_scroll(self.scroll.clone())
            .size_full()
            .into_any_element()
        };
        let body = div().flex_1().min_h_0().relative().child(list).child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&self.scroll)),
        );
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .p(tokens::SPACE_MD)
            .border_l_1()
            .border_color(palette::border())
            .child(
                section(
                    rox_i18n::t!("smart-playlist-matched-tracks"),
                    Some(
                        div()
                            .text_xs()
                            .text_color(palette::text())
                            .child(count)
                            .into_any_element(),
                    ),
                    body,
                )
                .flex_1()
                .min_h_0(),
            )
    }

    fn preview_rows(&self, range: std::ops::Range<usize>, cx: &App) -> Vec<Div> {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        range
            .filter_map(|i| {
                let row = *self.matched.get(i)? as usize;
                // A scan swaps the projection under an open window, so a stale
                // row draws as nothing.
                if row >= projection.len() || projection.is_dead(row as u32) {
                    return None;
                }
                let view = projection.resolve(row as u32);
                Some(preview_row(view.title, view.artist))
            })
            .collect()
    }

    fn footer(&self, savable: bool, cx: &mut Context<Self>) -> Div {
        let hint = if savable {
            kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to save".into()),
            ])
            .text_xs()
            .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(rox_i18n::t!("smart-playlist-name-to-save"))
                .into_any_element()
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
                    .child(small_button(
                        "Save",
                        icons::CHECK,
                        !savable,
                        cx.listener(|this, _, window, cx| this.commit(window, cx)),
                    ))
                    .child(small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

fn preview_row(title: &str, artist: &str) -> Div {
    div()
        .h(ROW_H)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_MD)
        .text_xs()
        .child(div().flex_1().min_w_0().truncate().child(title.to_string()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette::text_muted())
                .child(artist.to_string()),
        )
}

impl Focusable for SmartPlaylistWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

impl Render for SmartPlaylistWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let savable = self.savable(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.commit(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .bg(palette::bg_elevated())
                    .child(self.controls(cx))
                    .child(self.preview(cx)),
            )
            .child(self.footer(savable, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_prefix_says_it_reads_as_text() {
        let note = query_note("ac:dc").expect("an unknown prefix earns a note");
        assert!(note.contains("ac:"), "{note}");
    }

    #[test]
    fn the_real_fields_pass_quietly() {
        assert!(query_note("").is_none());
        assert!(query_note("stronger").is_none());
        assert!(query_note("year:1997 rating:>=4 added:<90d folder:live").is_none());
        assert!(query_note("artist:\"ac:dc\"").is_none());
    }
}
