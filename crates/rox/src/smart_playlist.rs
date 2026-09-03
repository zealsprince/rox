//! The smart playlist editor: a window over one saved query. The
//! definition is in the left column (a name, the query itself in a search
//! box that uses the same syntax and offers the same completions as the
//! library's, an optional sort and cap), and what that definition
//! currently takes fills the right.
//!
//! The preview is the point of the window. A saved query is a promise
//! about rows nobody can see yet, so the editor evaluates on every change
//! and shows the tracks before anything is saved. Evaluation is one
//! projection pass, the same one the panel runs on refresh, and it keeps
//! the rows rather than the tracks: row text resolves through
//! the projection per visible row, so a query that takes the whole
//! library costs one pass and nothing per row after it. Nothing here runs
//! a pass per frame.
//!
//! The structured filter is passed through untouched: it has no controls here
//! (the filter panel builds those), and an edit that dropped it silently
//! would lose work the query text can't express.

use gpui::{
    actions, div, prelude::*, px, size, uniform_list, App, Bounds, Context, Div, Entity,
    FocusHandle, Focusable, KeyBinding, MouseButton, MouseDownEvent, Pixels, SharedString,
    Subscription, UniformListScrollHandle, Window,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::Sizable;

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::playlists::SmartDef;
use rox_library::projection::{SortKey, QUERY_FIELDS};
use rox_panel_api::panel::AppState;
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::suggest;
use rox_panel_kit::ui::{self as settings_ui, checkbox, kbd_line, section, small_button, Seg};
use rox_services::backdrop::WindowBackdrop;

/// The definition column's width: room for a label and its field, and no
/// more, so every pixel the window grows by goes to the preview.
const CONTROLS_W: Pixels = px(340.);

/// The label column inside a field row, and the indent a note under one
/// takes to line up with the control rather than the label.
const LABEL_W: Pixels = px(64.);

/// One preview row's height. The list is a `uniform_list`, so every row
/// has to agree on it.
const ROW_H: Pixels = px(22.);

/// The sorts a smart playlist can ask for, in the order the dropdown lists
/// them. Its own list rather than the library table's columns: a saved
/// query orders by the handful of fields people build lists around, not
/// every column a table can show.
///
/// A function rather than a `const`: the labels resolve through `t!`,
/// which isn't const-evaluable, so the list gets rebuilt each call rather
/// than baked in for one locale.
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

/// The label a sort key reads as in the dropdown.
fn sort_label(sort: Option<SortKey>) -> SharedString {
    sorts()
        .into_iter()
        .find(|(_, key)| *key == sort)
        .map(|(label, _)| label)
        .unwrap_or_else(|| rox_i18n::t!("smart-playlist-sort-default"))
}

actions!(smart_playlist, [Save]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "SmartPlaylist";

/// The editor's save binding; call once at startup. It's bound on the
/// window root, so Enter saves wherever focus is (a dropdown, the
/// checkbox, the preview) and not only in the field that happens to hold
/// it. The inputs still see the key first, since their own binding is
/// deeper along the focus path: a single-line input propagates it up to
/// here, and an open suggestion menu swallows it, so Enter takes the
/// suggestion first and saves on the next press.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Save, Some(CONTEXT))]);
}

/// What's off about the query, when something is. The query language
/// never fails to parse: an unknown `foo:` prefix quietly falls back to a
/// plain text term (the rule [`rox_library::projection::parse_query`]
/// follows), which is exactly the mistake worth catching before somebody
/// saves a playlist that matches nothing.
fn query_note(query: &str) -> Option<SharedString> {
    let unknown = query
        .split_whitespace()
        // A quoted value is a value, not a prefix. `artist:"ac:dc"`
        // arrives split across tokens, and neither half is a claim about
        // a field.
        .filter(|token| !token.contains('"'))
        .find_map(|token| {
            let (name, _) = token.split_once(':')?;
            // A leading hyphen negates the term, so the field name is
            // what follows it: `-genre:rock` is a genre pin like any
            // other and shouldn't read as an unknown "-genre". The note
            // still quotes the token as typed.
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

/// Open the editor. `id` names an existing smart playlist to edit; None
/// starts a new one.
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
        // The floor keeps the two columns apart: the definition holds its
        // width, so a shrinking window eats into the preview and stops
        // before either is unusable.
        Some(settings_ui::MIN_SIZE),
        move |window, cx| cx.new(|cx| SmartPlaylistWindow::new(state, id, window, cx)),
    );
}

struct SmartPlaylistWindow {
    state: AppState,
    /// The playlist being edited, None while making a new one.
    id: Option<i64>,
    name: Entity<InputState>,
    query: Entity<SearchBox>,
    limit: Entity<InputState>,
    sort: Option<SortKey>,
    descending: bool,
    /// The filter the loaded definition held, passed straight back
    /// through on save. Nothing here edits it.
    filter: rox_library::projection::FilterSet,
    /// The projection rows the current definition takes, re-evaluated on
    /// every change and never on a frame. Rows rather than tracks: the
    /// preview resolves the few it draws off these.
    matched: Vec<u32>,
    /// The save already ran. One Enter press can reach [`Self::commit`]
    /// twice (the focused input's binding and the window's, which the
    /// input propagates to), and a second save would file a second
    /// playlist.
    saved: bool,
    scroll: UniformListScrollHandle,
    backdrop: WindowBackdrop,
    _query_events: Subscription,
    _name_events: Subscription,
    _limit_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own wake
    /// on a new bake.
    _backdrop_changed: Subscription,
}

impl SmartPlaylistWindow {
    fn new(state: AppState, id: Option<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Editing loads what's saved; a new one opens empty.
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
        // The same completions the library's box gets, so the syntax is
        // learnable in the one place it's saved for good.
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
                // Enter in the query field saves, the way it does in the
                // name field.
                SearchEvent::Submitted => this.commit(window, cx),
                _ => {}
            },
        );
        let _name_events = cx.subscribe_in(
            &name,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                // The name gates the save, so the button follows it
                // keystroke by keystroke.
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

    /// The definition the fields currently spell out.
    fn definition(&self, cx: &App) -> SmartDef {
        let limit = self.limit.read(cx).value().trim().parse::<u32>().ok();
        SmartDef {
            query: self.query.read(cx).query().to_string(),
            filter: self.filter.clone(),
            sort: self.sort.map(|key| (key, self.descending)),
            // A zero cap is nobody's intent, so it reads as no cap at all.
            limit: limit.filter(|&n| n > 0),
        }
    }

    /// Re-evaluate against the catalog and repaint the preview. One
    /// projection pass, run on a change rather than on a frame.
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

    /// Whether the definition can be saved as it stands. A blank name is
    /// the only thing that blocks it: the query syntax has no invalid
    /// state, and a query that takes nothing is a real thing to save.
    fn savable(&self, cx: &App) -> bool {
        !self.name.read(cx).value().trim().is_empty()
    }

    /// Save and close. A blank name does nothing, which the footer shows
    /// in place of the shortcut so the block isn't silent.
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

    /// One labeled row of the form.
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

    /// The definition column: the fields, and what's off about the query
    /// under the field it's about.
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
                        // Indented past the label column, so the note is
                        // under the box it's about.
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
                    // The direction drops to its own line rather than
                    // pushing out of the column when a sort name runs long.
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
                    // Direction only means something once a sort is picked.
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

    /// The preview column: the count over the tracks the definition takes.
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

    /// The visible slice of the preview. Row text resolves through the
    /// projection per visible row, so a query that takes the whole library
    /// costs only what shows.
    fn preview_rows(&self, range: std::ops::Range<usize>, cx: &App) -> Vec<Div> {
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        range
            .filter_map(|i| {
                let row = *self.matched.get(i)? as usize;
                // A scan swaps the projection under an open window, which
                // leaves the rows this one kept pointing past the end of
                // the new one. A stale row draws as nothing rather than
                // reading off the end.
                if row >= projection.len() || projection.is_dead(row as u32) {
                    return None;
                }
                let view = projection.resolve(row as u32);
                Some(preview_row(view.title, view.artist))
            })
            .collect()
    }

    /// The window's own actions: the save, and the shortcut for it.
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

/// One preview row: a track the query took, the title beside who made it.
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
                    // The body's own surface, a second elevated layer over the
                    // window's, the same as the settings page. The backdrop
                    // reads through two layers everywhere.
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
        // A quoted value is a value, never a claim about a field.
        assert!(query_note("artist:\"ac:dc\"").is_none());
    }
}
