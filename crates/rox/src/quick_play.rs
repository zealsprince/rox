//! The quick-play modal: Ctrl/Cmd+P or Ctrl/Cmd+F drops a search box over
//! the workspace to jump straight to a track. Typing filters the whole
//! catalog through the projection's search, enter or a click queues from
//! the picked track in result order, escape closes. A view over the same
//! shared catalog and player the panels use, hosted as an overlay instead
//! of a dock item; the workspace owns one at most and drops it on dismiss.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, uniform_list, App, Context, DismissEvent, Div, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{
    Input, InputEvent, InputState, MoveDown, MovePageDown, MovePageUp, MoveUp,
};

use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::panels::library::{fmt_ms, LibraryEvent, QUEUE_CAP};

/// One result row's height; the list is a uniform_list, so every row
/// must agree on it.
const ROW_H: f32 = 30.;

/// How many rows show before the list scrolls.
const VISIBLE_ROWS: usize = 14;

/// How far page up and page down step, most of a full view.
const PAGE_ROWS: isize = 10;

pub struct QuickPlay {
    state: AppState,
    /// The query editor; `query` mirrors its value via change events.
    input: Entity<InputState>,
    query: String,
    /// Projection rows matching the query, in the library panel's view
    /// order: canonical browse order while empty, search order otherwise.
    hits: Arc<Vec<u32>>,
    /// The highlighted row, what enter plays.
    selected: usize,
    scroll: UniformListScrollHandle,
    /// A failed play, shown until the next query change.
    error: Option<SharedString>,
    _input_events: Subscription,
    _library_changed: Subscription,
}

impl EventEmitter<DismissEvent> for QuickPlay {}

impl Focusable for QuickPlay {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl QuickPlay {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search the library"));
        let _input_events = cx.subscribe_in(&input, window, Self::on_input_event);
        // A scan finishing mid-search would leave the hits pointing into
        // the old projection; recompute over the new one.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut QuickPlay, _, _: &LibraryEvent, cx| this.refresh(cx),
        );
        let mut this = QuickPlay {
            state,
            input,
            query: String::new(),
            hits: Arc::new(Vec::new()),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            error: None,
            _input_events,
            _library_changed,
        };
        this.refresh(cx);
        this
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

    fn on_input_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.query = input.read(cx).value().to_string();
                self.refresh(cx);
            }
            InputEvent::PressEnter { .. } => self.play(self.selected, cx),
            _ => {}
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
        rows.into_iter()
            .map(|(ix, title, sub, time)| {
                div()
                    .h(px(ROW_H))
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
                    .child(div().flex_1().truncate().child(title))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_color(palette::text_secondary())
                            .child(sub),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(time),
                    )
            })
            .collect()
    }
}

impl Render for QuickPlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let len = self.hits.len();
        let list_h = px(ROW_H * len.clamp(1, VISIBLE_ROWS) as f32);
        let this = cx.entity().downgrade();
        let list = if len == 0 {
            div()
                .h(list_h)
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(if self.query.is_empty() {
                    "the library is empty"
                } else {
                    "no matches"
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
            // they reach it.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_selected(-1, cx)))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_selected(1, cx)))
            .capture_action(cx.listener(|this, _: &MovePageUp, _, cx| {
                this.move_selected(-PAGE_ROWS, cx)
            }))
            .capture_action(cx.listener(|this, _: &MovePageDown, _, cx| {
                this.move_selected(PAGE_ROWS, cx)
            }))
            // The input propagates escape when it has nothing of its own
            // (IME, context menu) to close, so it lands here.
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
                    .child(Input::new(&self.input).w_full()),
            )
            .child(list)
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
