//! The quick-play modal: Ctrl/Cmd+P or Ctrl/Cmd+F drops a search box over
//! the workspace to jump straight to a track. Typing filters the whole
//! catalog through the projection's search, enter or a click queues from
//! the picked track in result order, escape closes. A view over the same
//! shared catalog and player the panels use, hosted as an overlay instead
//! of a dock item; the workspace owns one at most and drops it on dismiss.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, svg, uniform_list, Action, App, Context, DismissEvent, Div, Entity,
    EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window,
};
use gpui_component::input::{MoveDown, MovePageDown, MovePageUp, MoveUp};
use rox_library::projection::QUERY_FIELDS;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState};
use crate::panels::library::{fmt_ms, LibraryEvent, QUEUE_CAP};
use crate::search::{SearchBox, SearchEvent};
use crate::settings::{QuickPlayConfig, Settings};
use crate::suggest;

/// One result row's height; the list is a uniform_list, so every row must
/// agree on it. Comfortable rows run taller.
const ROW_H: f32 = 30.;
const ROW_H_COMFORTABLE: f32 = 40.;

/// What the subtitle line under the title adds to a row's height.
const SUBTITLE_H: f32 = 14.;

/// How many rows show before the list scrolls.
const VISIBLE_ROWS: usize = 14;

/// How far page up and page down step, most of a full view.
const PAGE_ROWS: isize = 10;

pub struct QuickPlay {
    state: AppState,
    /// The query editor, the shared search box; `query` mirrors its value
    /// via change events.
    search: Entity<SearchBox>,
    query: String,
    /// Projection rows matching the query, in the library panel's view
    /// order: canonical browse order while empty, search order otherwise.
    hits: Arc<Vec<u32>>,
    /// The highlighted row, what enter plays.
    selected: usize,
    scroll: UniformListScrollHandle,
    /// A failed play, shown until the next query change.
    error: Option<SharedString>,
    /// The result list's appearance knobs, mirrored from settings and
    /// written back on every edit.
    config: QuickPlayConfig,
    /// Whether the inline config panel is open, beside the search box.
    show_config: bool,
    _input_events: Subscription,
    _library_changed: Subscription,
}

impl EventEmitter<DismissEvent> for QuickPlay {}

impl Focusable for QuickPlay {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }
}

impl QuickPlay {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| SearchBox::new("Search the library", "", window, cx));
        let _input_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // A scan finishing mid-search would leave the hits pointing into
        // the old projection; recompute over the new one, suggestions
        // included.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut QuickPlay, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.attach_suggestions(cx);
                this.refresh(cx);
            },
        );
        let mut this = QuickPlay {
            state,
            search,
            query: String::new(),
            hits: Arc::new(Vec::new()),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            error: None,
            config: Settings::load().quick_play,
            show_config: false,
            _input_events,
            _library_changed,
        };
        this.attach_suggestions(cx);
        this.refresh(cx);
        this
    }

    /// Point the search box's suggestion menu at the current projection;
    /// at open and again whenever a scan lands a new one.
    fn attach_suggestions(&self, cx: &mut Context<Self>) {
        let provider = {
            let library = self.state.library.read(cx);
            suggest::query_provider(library.projection())
        };
        self.search
            .update(cx, |search, cx| search.set_completions(provider, cx));
    }

    /// Each result row's height, taller when comfortable rows are on and
    /// again when the subtitle line shows.
    fn row_h(&self) -> f32 {
        let base = if self.config.comfortable {
            ROW_H_COMFORTABLE
        } else {
            ROW_H
        };
        if self.config.show_subtitle {
            base + SUBTITLE_H
        } else {
            base
        }
    }

    /// Let the search box's suggestion menu take an action first; true
    /// when it was open and consumed it.
    fn menu_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.search
            .update(cx, |search, cx| search.menu_action(action, window, cx))
    }

    /// Flip the config panel open or shut.
    fn toggle_config(&mut self, cx: &mut Context<Self>) {
        self.show_config = !self.show_config;
        cx.notify();
    }

    /// Change one config knob, persist it, repaint.
    fn edit_config(&mut self, edit: impl FnOnce(&mut QuickPlayConfig), cx: &mut Context<Self>) {
        edit(&mut self.config);
        let config = self.config.clone();
        Settings::update(move |s| s.quick_play = config);
        cx.notify();
    }

    /// Recompute the hits for the current query and reset the highlight
    /// to the top.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let hits = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) if !self.query.is_empty() => {
                    Arc::new(projection.search(&self.query))
                }
                Some(_) => library.order(),
                None => Arc::new(Vec::new()),
            }
        };
        self.hits = hits;
        self.selected = 0;
        self.error = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                self.query = search.read(cx).query().to_string();
                self.refresh(cx);
            }
            SearchEvent::Submitted => self.play(self.selected, cx),
            // The box's escape ladder ends here: the query is already
            // empty, so escape closes the modal.
            SearchEvent::Dismissed => cx.emit(DismissEvent),
            SearchEvent::FocusChanged => {}
        }
    }

    /// Step the highlight, clamped to the list; the scroll follows only
    /// when the row leaves the view.
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.hits.len();
        if len == 0 {
            return;
        }
        let ix = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
        if ix == self.selected {
            return;
        }
        self.selected = ix;
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Queue the picked hit and what follows it in the current result
    /// order, same as a double click in the library, then dismiss.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.hits.len() {
            return;
        }
        let result = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = self.hits[ix..]
                .iter()
                .take(QUEUE_CAP)
                .map(|&row| projection.db_id[row as usize])
                .collect();
            library.paths_for(&ids)
        };
        match result {
            Ok(paths) => {
                self.state
                    .player
                    .update(cx, |player, cx| player.play(paths, cx));
                cx.emit(DismissEvent);
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    /// The visible slice of the hit list. Row text resolves through the
    /// projection per visible row, so a huge library costs only what
    /// shows.
    fn hit_rows(&self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let rows: Vec<(usize, SharedString, SharedString, SharedString)> = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return Vec::new();
            };
            range
                .filter_map(|ix| {
                    let row = *self.hits.get(ix)?;
                    let v = projection.resolve(row);
                    let sub = match (v.artist.is_empty(), v.album.is_empty()) {
                        (false, false) => format!("{} - {}", v.artist, v.album),
                        (false, true) => v.artist.to_string(),
                        (true, false) => v.album.to_string(),
                        (true, true) => String::new(),
                    };
                    Some((
                        ix,
                        SharedString::from(v.title.to_string()),
                        SharedString::from(sub),
                        SharedString::from(fmt_ms(v.duration_ms)),
                    ))
                })
                .collect()
        };
        let row_h = self.row_h();
        rows.into_iter()
            .map(|(ix, title, sub, time)| {
                div()
                    // Fills the list's width so a long title truncates
                    // inside the modal instead of running the row wide, and
                    // the duration stays pinned to the right edge.
                    .w_full()
                    .h(px(row_h))
                    .px(tokens::SPACE_SM)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .cursor_pointer()
                    .when(ix == self.selected, |d| {
                        d.bg(palette::alpha(palette::accent(), 0x26))
                    })
                    .hover(|d| d.bg(palette::bg_control_hover_opaque()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.play(ix, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(div().w_full().truncate().child(title))
                            .when(self.config.show_subtitle, |d| {
                                d.child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_xs()
                                        .text_color(palette::text_secondary())
                                        .child(sub),
                                )
                            }),
                    )
                    .when(self.config.show_duration, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(palette::text_muted())
                                .child(time),
                        )
                    })
            })
            .collect()
    }

    /// The footer of field chips that makes the query syntax visible:
    /// each one appends its `field:` to the query and pops the value
    /// suggestions, so narrowing is one click plus picking a value.
    fn hint_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child("Narrow By")
            .children(QUERY_FIELDS.iter().map(|(name, _)| {
                let term = SharedString::from(format!("{name}:"));
                div()
                    .px(tokens::SPACE_XS)
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_control())
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control_hover_opaque()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let term = term.clone();
                            move |this, _, window, cx| {
                                this.search
                                    .update(cx, |search, cx| search.append_term(&term, window, cx));
                            }
                        }),
                    )
                    .child(term)
            }))
    }

    /// The settings button beside the search box: a sliders glyph that
    /// opens the config panel, tinted while it is open.
    fn config_button(&self, cx: &mut Context<Self>) -> Div {
        let on = self.show_config;
        div()
            .flex_none()
            .p(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(on, |d| d.bg(palette::bg_control_active()))
            .when(!on, |d| d.hover(|d| d.bg(palette::bg_control())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_config(cx)),
            )
            .child(svg().path(icons::SLIDERS).size(px(16.)).text_color(if on {
                palette::text()
            } else {
                palette::text_muted()
            }))
    }

    /// The inline config panel that drops under the search row when the
    /// settings button is on: the modal's appearance knobs, each writing
    /// straight through to settings.
    fn config_panel(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .pt(tokens::SPACE_SM)
            .mt(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .child(panel::setting_row(
                "Subtitle",
                Some("Show the artist and album under each result"),
                panel::toggle(
                    self.config.show_subtitle,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_subtitle = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Duration",
                Some("Show each result's length on the right"),
                panel::toggle(
                    self.config.show_duration,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_duration = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Comfortable Rows",
                Some("Give each result more height"),
                panel::toggle(
                    self.config.comfortable,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.comfortable = on, cx);
                    },
                    cx,
                ),
            ))
    }
}

impl Render for QuickPlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let len = self.hits.len();
        let list_h = px(self.row_h() * len.clamp(1, VISIBLE_ROWS) as f32);
        let this = cx.entity().downgrade();
        let list = if len == 0 {
            div()
                .h(list_h)
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(if self.query.is_empty() {
                    "The library is empty"
                } else {
                    "No matches"
                })
                .into_any_element()
        } else {
            uniform_list("quick-play-hits", len, move |range, _, cx| {
                this.upgrade()
                    .map(|this| this.update(cx, |this, cx| this.hit_rows(range, cx)))
                    .unwrap_or_default()
            })
            .track_scroll(self.scroll.clone())
            .h(list_h)
            .w_full()
            .into_any_element()
        };
        div()
            .w(px(560.))
            .flex()
            .flex_col()
            .bg(palette::bg_menu_opaque())
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            // Scopes the workspace's playback key bindings out while the
            // modal is up, so space and arrows work the query and the
            // list instead.
            .key_context("SearchInput")
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            // The input binds up/down (and page keys) to its own cursor
            // actions and swallows them without propagating on a single
            // line, so the list takes them in the capture phase before
            // they reach it - unless the suggestion menu is open, which
            // gets them first so it stays navigable.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.menu_action(Box::new(MoveUp), window, cx) {
                    this.move_selected(-1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.menu_action(Box::new(MoveDown), window, cx) {
                    this.move_selected(1, cx);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &MovePageUp, _, cx| this.move_selected(-PAGE_ROWS, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MovePageDown, _, cx| this.move_selected(PAGE_ROWS, cx)),
            )
            // The search box handles escape while it has focus (its clear
            // then dismiss ladder); this catches an escape from anywhere
            // else in the modal.
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .child(
                div()
                    .p(tokens::SPACE_SM)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.search.update(cx, |search, cx| search.element(cx))),
                            )
                            .child(self.config_button(cx)),
                    )
                    .when(self.show_config, |d| d.child(self.config_panel(cx))),
            )
            .child(list)
            .child(self.hint_row(cx))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .border_t_1()
                        .border_color(palette::border())
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }
}
