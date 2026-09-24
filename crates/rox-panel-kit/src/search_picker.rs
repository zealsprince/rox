//! The searchable dropdown the font, language and icon pickers share: a
//! [`select_field`](crate::ui::select_field) over a popover with a search box,
//! since a `PopupMenu` is built once and can't be filtered. It keeps rox's own
//! menu chrome: the library's dropdown paints on the structural background,
//! which vanishes once surfaces go translucent.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, Focusable as _, MouseButton, Pixels, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::popover::Popover;
use gpui_component::{Icon, Sizable as _};

use rox_design::assets::icons;
use rox_design::{palette, tokens};

use crate::ui as settings_ui;

/// Takes the value a row sets, None for the head row that clears to default.
type Pick = Rc<dyn Fn(Option<String>, &mut App)>;

type Commit = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

const ROW_H: Pixels = px(22.);

const ROWS: usize = 12;

/// Wider than the field, which truncates a long label.
const LIST_W: Pixels = px(240.);

/// A `None` value is the head row that clears the override; an active query
/// filters it out.
#[derive(Clone, PartialEq)]
pub struct PickRow {
    pub label: SharedString,
    pub value: Option<SharedString>,
    /// Extra search terms, lowercase by contract: "canadian" finds English.
    pub terms: Vec<SharedString>,
    pub icon: Option<SharedString>,
}

/// The strings are the caller's because the caller knows whether they
/// translate.
// `use<..>` and the named `A` for the same reason as the crate root's
// `picker`.
#[allow(clippy::too_many_arguments)]
pub fn search_picker<P, A>(
    id: &'static str,
    rows: Arc<Vec<PickRow>>,
    label: SharedString,
    current: Option<SharedString>,
    placeholder: SharedString,
    empty: SharedString,
    apply: A,
    cx: &mut Context<P>,
) -> impl IntoElement + use<P, A>
where
    P: 'static,
    A: Fn(&mut P, Option<String>, &mut Context<P>) + 'static,
{
    let host = cx.entity().downgrade();
    SearchPicker {
        id,
        rows,
        label,
        current,
        placeholder,
        empty,
        pick: Rc::new(move |value, cx| {
            if let Some(host) = host.upgrade() {
                host.update(cx, |this, cx| apply(this, value, cx));
            }
        }),
    }
}

/// Kept in the element's keyed state, so a host builds a picker with one
/// call and owns nothing.
struct Search {
    input: Entity<InputState>,
    all: Arc<Vec<PickRow>>,
    hits: Vec<usize>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _events: Subscription,
}

impl Search {
    fn new(
        all: Arc<Vec<PickRow>>,
        placeholder: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let _events = cx.subscribe(&input, |this: &mut Self, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.filter(input.read(cx).value().as_ref());
                cx.notify();
            }
        });
        let hits = (0..all.len()).collect();
        Self {
            input,
            all,
            hits,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _events,
        }
    }

    /// The state outlives a locale switch, so take a changed row set.
    fn retarget(
        &mut self,
        all: &Arc<Vec<PickRow>>,
        placeholder: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.all == *all {
            return;
        }
        self.all = all.clone();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder.clone(), window, cx);
        });
        self.filter("");
    }

    fn filter(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        self.hits = if query.is_empty() {
            (0..self.all.len()).collect()
        } else {
            self.all
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    row.value.is_some()
                        && (row.label.to_lowercase().contains(&query)
                            || row.terms.iter().any(|term| term.contains(&query)))
                })
                .map(|(ix, _)| ix)
                .collect()
        };
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Opens on an empty query with the current value under the cursor.
    fn reset(
        &mut self,
        current: &Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.filter("");
        self.selected = self
            .hits
            .iter()
            .position(|ix| self.all[*ix].value == *current)
            .unwrap_or(0);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() as isize - 1;
        let next = (self.selected as isize + delta).clamp(0, last) as usize;
        if next == self.selected {
            return;
        }
        self.selected = next;
        // Non-strict, so a row already in view doesn't jerk to the top.
        self.scroll.scroll_to_item(next, ScrollStrategy::Top);
        cx.notify();
    }

    fn picked(&self) -> Option<Option<String>> {
        let row = self.all.get(*self.hits.get(self.selected)?)?;
        Some(row.value.as_ref().map(|value| value.to_string()))
    }
}

#[derive(IntoElement)]
struct SearchPicker {
    id: &'static str,
    rows: Arc<Vec<PickRow>>,
    label: SharedString,
    current: Option<SharedString>,
    placeholder: SharedString,
    empty: SharedString,
    pick: Pick,
}

impl RenderOnce for SearchPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let rows = self.rows;
        let search = window.use_keyed_state((self.id, 1usize), cx, {
            let rows = rows.clone();
            let placeholder = self.placeholder.clone();
            move |window, cx| Search::new(rows, placeholder, window, cx)
        });
        search.update(cx, |state, cx| {
            let placeholder = self.placeholder.clone();
            state.retarget(&rows, &placeholder, window, cx);
        });
        let current = self.current;
        let focus = search.read(cx).input.read(cx).focus_handle(cx);
        let pick = self.pick;
        let empty = self.empty;

        Popover::new((self.id, 0usize))
            // rox draws the surface itself.
            .appearance(false)
            .track_focus(&focus)
            .trigger(settings_ui::select_field(self.id, self.label, false))
            .on_open_change({
                let search = search.clone();
                let current = current.clone();
                move |open, window, cx| {
                    if *open {
                        search.update(cx, |this, cx| this.reset(&current, window, cx));
                    }
                }
            })
            .content(move |_, _, cx| {
                let popover = cx.entity();
                let commit: Commit = Rc::new({
                    let pick = pick.clone();
                    move |value, window, cx| {
                        pick(value, cx);
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                    }
                });
                let count = search.read(cx).hits.len();
                let body = if count == 0 {
                    div()
                        .h(ROW_H * 3.)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(palette::text_muted())
                        .child(empty.clone())
                        .into_any_element()
                } else {
                    uniform_list("search-picker-rows", count, {
                        let search = search.clone();
                        let current = current.clone();
                        let commit = commit.clone();
                        move |range, _, cx| {
                            let state = search.read(cx);
                            range
                                .map(|ix| {
                                    let row =
                                        state.hits.get(ix).and_then(|hit| state.all.get(*hit));
                                    let Some(row) = row.cloned() else {
                                        return div();
                                    };
                                    let picked = row.value == current;
                                    let value = row.value.map(|value| value.to_string());
                                    let commit = commit.clone();
                                    row_body(row.label, row.icon, picked, ix == state.selected)
                                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                            commit(value.clone(), window, cx)
                                        })
                                })
                                .collect()
                        }
                    })
                    .track_scroll(search.read(cx).scroll.clone())
                    .h(ROW_H * count.min(ROWS) as f32)
                    .w_full()
                    .into_any_element()
                };
                div()
                    .w(LIST_W)
                    .flex()
                    .flex_col()
                    .bg(palette::bg_menu_opaque())
                    .rounded(tokens::RADIUS)
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    // A one-line input never hands the arrows on, so take
                    // them on the way down.
                    .capture_action({
                        let search = search.clone();
                        cx.listener(move |_, _: &MoveUp, _, cx| {
                            search.update(cx, |this, cx| this.step(-1, cx));
                        })
                    })
                    .capture_action({
                        let search = search.clone();
                        cx.listener(move |_, _: &MoveDown, _, cx| {
                            search.update(cx, |this, cx| this.step(1, cx));
                        })
                    })
                    .capture_action({
                        let search = search.clone();
                        let pick = pick.clone();
                        // Not through `commit`: this runs inside the popover's
                        // update, and `commit`'s `popover.update` would nest
                        // and panic.
                        cx.listener(move |this, _: &Enter, window, cx| {
                            if let Some(value) = search.read(cx).picked() {
                                pick(value, cx);
                                this.dismiss(window, cx);
                            }
                        })
                    })
                    .child(
                        div()
                            .p(tokens::SPACE_XS)
                            .border_b_1()
                            .border_color(palette::border())
                            .child(
                                Input::new(&search.read(cx).input)
                                    .small()
                                    .w_full()
                                    .cleanable(true)
                                    .prefix(
                                        Icon::default()
                                            .path(icons::SEARCH)
                                            .small()
                                            .text_color(palette::text_muted()),
                                    ),
                            ),
                    )
                    .child(div().p(tokens::SPACE_XS).child(body))
            })
    }
}

/// The tick trails rather than leads so unpicked rows don't all get its indent.
fn row_body(
    label: SharedString,
    icon: Option<SharedString>,
    picked: bool,
    selected: bool,
) -> gpui::Div {
    div()
        .w_full()
        .h(ROW_H)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .when(selected, |d| d.bg(palette::bg_menu_hover()))
        .hover(|d| d.bg(palette::bg_menu_hover()))
        // Menu-scale glyph, matching a panel settings menu item.
        .when_some(icon, |d, path| d.child(Icon::default().path(path)))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label),
        )
        .when(picked, |d| {
            d.child(
                svg()
                    .path(icons::CHECK)
                    .size(px(12.))
                    .flex_none()
                    .text_color(palette::accent()),
            )
        })
}
