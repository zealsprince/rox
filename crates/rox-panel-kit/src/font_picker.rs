//! The font-family picker: the field the app settings window and every
//! panel's Appearance page drop their typeface list from.
//!
//! It draws the shared [`select_field`](crate::ui::select_field) and hangs
//! its own popover under it rather than a `PopupMenu`, because the list
//! needs a search box and a menu's items are built once when it opens. A
//! machine carries hundreds of families, so scrolling for one by eye is no
//! way to find it; the box filters as you type and the arrows walk what's
//! left. The popover keeps rox's own menu chrome for the same reason the
//! rest of the settings windows do: the widget library's dropdown paints on
//! the structural background, which drops out entirely once surfaces go
//! translucent, and a see-through font list is unreadable.

use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gpui::{
    div, prelude::*, px, svg, uniform_list, AnyElement, App, Context, Entity, Focusable as _,
    MouseButton, Pixels, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    Window,
};
use gpui_component::input::{Enter, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::popover::Popover;
use gpui_component::{Icon, Sizable as _};

use rox_design::assets::icons;
use rox_design::{palette, tokens};

use crate::ui as settings_ui;

/// The head of the list, the row that clears the override so the text
/// falls back to whatever the layer above sets.
const DEFAULT_LABEL: &str = "Default";

/// The host's own handler, wrapped so the element can carry it: takes the
/// family a row sets, None for the Default head.
type Pick = Rc<dyn Fn(Option<String>, &mut App)>;

/// A [`Pick`] that closes the list behind it, what a click or an Enter
/// runs.
type Commit = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

/// One row's height. The list is a `uniform_list`, so every row agrees on
/// it.
const ROW_H: Pixels = px(22.);

/// How many rows show before the list scrolls.
const ROWS: usize = 12;

/// The list runs wider than the field it drops from: the field truncates a
/// long family name, so the list is where the whole name has to fit.
const LIST_W: Pixels = px(240.);

/// A font-family picker: the shared field over the installed families,
/// with a Default at the head that clears the override back to the app
/// font. `current` is the panel's stored family, None meaning inherit.
pub fn font_picker<P: 'static>(
    id: &'static str,
    current: Option<String>,
    apply: impl Fn(&mut P, Option<String>, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
    let host = cx.entity().downgrade();
    FontPicker {
        id,
        current,
        pick: Rc::new(move |font, cx| {
            if let Some(host) = host.upgrade() {
                host.update(cx, |this, cx| apply(this, font, cx));
            }
        }),
    }
}

/// One row of the list: what it reads as, and the family it sets.
#[derive(Clone)]
struct FontRow {
    label: SharedString,
    /// None on the Default head, which clears the override.
    value: Option<SharedString>,
}

/// The installed families, Default at the head, enumerated and sorted
/// once and shared from there. They don't change over a session, and this
/// used to run on every settings render, slider scrubs included, where
/// re-listing and re-sorting every font each frame was pure waste.
fn families(cx: &mut App) -> Arc<Vec<FontRow>> {
    static FONTS: OnceLock<Arc<Vec<FontRow>>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut names = cx.text_system().all_font_names();
            names.sort();
            names.dedup();
            let mut rows = vec![FontRow {
                label: DEFAULT_LABEL.into(),
                value: None,
            }];
            rows.extend(names.into_iter().map(|name| {
                let name = SharedString::from(name);
                FontRow {
                    label: name.clone(),
                    value: Some(name),
                }
            }));
            Arc::new(rows)
        })
        .clone()
}

/// What the picker keeps between frames: the search box, which families
/// the query left, and the row the arrows have landed on. It lives in the
/// element's own keyed state, so a host builds a picker the way it builds
/// a toggle, by calling [`font_picker`], and owns nothing.
struct FontSearch {
    input: Entity<InputState>,
    all: Arc<Vec<FontRow>>,
    /// Indices into `all`, in list order: what the query left.
    hits: Vec<usize>,
    /// Which hit the arrows and Enter act on.
    selected: usize,
    scroll: UniformListScrollHandle,
    _events: Subscription,
}

impl FontSearch {
    fn new(all: Arc<Vec<FontRow>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search fonts"));
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

    /// The families a query keeps: case-insensitive, matched anywhere in
    /// the name, the way the library search matches. The Default head
    /// stays out of an active query, since nothing about it is a family
    /// someone would be typing toward.
    fn filter(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        self.hits = if query.is_empty() {
            (0..self.all.len()).collect()
        } else {
            self.all
                .iter()
                .enumerate()
                .filter(|(_, row)| row.value.is_some() && row.label.to_lowercase().contains(&query))
                .map(|(ix, _)| ix)
                .collect()
        };
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Opening starts clean: an empty box, the whole list, and the family
    /// that's already set under the cursor, so the list opens showing what
    /// it's about to replace.
    fn reset(
        &mut self,
        current: &Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.filter("");
        // Through `hits`, like every other read of `selected`: an empty
        // query leaves the two lined up, but nothing here should depend
        // on that.
        self.selected = self
            .hits
            .iter()
            .position(|ix| self.all[*ix].value == *current)
            .unwrap_or(0);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    /// Walk the list by `delta` rows, stopping at either end.
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
        // Non-strict, so a row already in view doesn't jerk the list to
        // the top under it.
        self.scroll.scroll_to_item(next, ScrollStrategy::Top);
        cx.notify();
    }

    /// The family the highlighted row sets, None when the query matched
    /// nothing and there's no row to take.
    fn picked(&self) -> Option<Option<String>> {
        let row = self.all.get(*self.hits.get(self.selected)?)?;
        Some(row.value.as_ref().map(|name| name.to_string()))
    }
}

/// [`font_picker`]'s element. It builds its own state on first render, so
/// the function stays a plain call in a settings row.
#[derive(IntoElement)]
struct FontPicker {
    id: &'static str,
    current: Option<String>,
    pick: Pick,
}

impl RenderOnce for FontPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let all = families(cx);
        let search = window.use_keyed_state((self.id, 1usize), cx, move |window, cx| {
            FontSearch::new(all, window, cx)
        });
        let current: Option<SharedString> = self.current.map(SharedString::from);
        let label = current.clone().unwrap_or_else(|| DEFAULT_LABEL.into());
        let focus = search.read(cx).input.read(cx).focus_handle(cx);
        let pick = self.pick;

        Popover::new((self.id, 0usize))
            // rox draws the surface itself, so the widget library's own
            // popover chrome would only stack a second card behind it.
            .appearance(false)
            // Opening hands focus to the box, so the list is searchable
            // without a click first.
            .track_focus(&focus)
            .trigger(settings_ui::select_field(self.id, label, false))
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
                    move |font, window, cx| {
                        pick(font, cx);
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
                        .child("No matches")
                        .into_any_element()
                } else {
                    uniform_list("fonts", count, {
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
                                    let value = row.value.map(|name| name.to_string());
                                    let commit = commit.clone();
                                    row_body(row.label, picked, ix == state.selected)
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
                    // The box only wires its own arrow handlers on a
                    // multi-line input, so on this one nothing would ever
                    // hand the arrows to the list. Take them on the way
                    // down instead.
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
                        let commit = commit.clone();
                        cx.listener(move |_, _: &Enter, window, cx| {
                            if let Some(font) = search.read(cx).picked() {
                                commit(font, window, cx);
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

/// One list row: the family name, a tick on the one that's set, and the
/// menu hover behind whichever the arrows or the pointer are on.
fn row_body(label: SharedString, picked: bool, selected: bool) -> gpui::Div {
    let mark: AnyElement = if picked {
        svg()
            .path(icons::CHECK)
            .size(px(12.))
            .flex_none()
            .text_color(palette::accent())
            .into_any_element()
    } else {
        div().size(px(12.)).flex_none().into_any_element()
    };
    div()
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
        .child(mark)
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label),
        )
}
