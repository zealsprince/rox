//! The chrome the settings windows share: the app settings window and
//! every panel's settings window draw their shell from one set, so a
//! page reads the same wherever it opens - the sidebar with its nav
//! rows, titled sections, group headers, the small header buttons, the
//! scalar slider, and the palette editor's role grid. Page content stays
//! with each window; only the shell lives here.

use gpui::{
    canvas, div, prelude::*, px, svg, AnyElement, App, Context, Div, MouseButton, MouseDownEvent,
    Pixels, Window,
};

use crate::design::palette::{self, ROLES};
use crate::design::tokens;
use crate::panel::{self, ScrubState};

/// The sidebar's width, room for a page name and no more.
pub const SIDEBAR_W: Pixels = px(140.);

/// The scalar sliders' strip width; the percent readout rides beside it.
pub const SLIDER_W: Pixels = px(140.);

/// The narrowest a color cell renders whole: the swatch, its gap, and
/// the longest role label.
pub const COLOR_CELL_MIN_W: Pixels = px(150.);

/// The gap between a page's sections, a step over the row rhythm so a
/// boundary reads as one.
pub const SECTION_GAP: Pixels = px(20.);

/// The floor under a settings window: the sidebar plus a colors row that
/// still fits its labels, and enough height for a page to breathe.
pub const MIN_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(560.),
    height: px(400.),
};

/// How many color-grid columns fit the page beside the sidebar: as many
/// whole cells as the window minus the sidebar and the body's insets
/// allows, two at the window floor up to four.
pub fn grid_columns(window: &Window) -> usize {
    let page_w = window.viewport_size().width - SIDEBAR_W - tokens::SPACE_MD * 2.;
    usize::clamp((page_w / COLOR_CELL_MIN_W) as usize, 2, 4)
}

/// The sidebar shell: the nav rows go in at the top; a window with
/// footer actions sinks them after its own spacer.
pub fn sidebar() -> Div {
    div()
        .w(SIDEBAR_W)
        .flex_none()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .p(tokens::SPACE_SM)
        .bg(palette::bg_panel())
        .border_r_1()
        .border_color(palette::border())
}

/// A sidebar row; the picked page reads like an active control.
pub fn nav_item<P: 'static>(
    label: &'static str,
    picked: bool,
    on_pick: impl Fn(&mut P, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .when(picked, |d| d.bg(palette::bg_control_active()))
        .when(!picked, |d| d.hover(|d| d.bg(palette::bg_menu_hover())))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| on_pick(this, cx)),
        )
        .child(label)
}

/// A header between setting groups, the palette listing's block names.
pub fn header(label: &'static str) -> Div {
    div()
        .pt(tokens::SPACE_SM)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
}

/// A titled section of a page: the name over a hairline, an optional
/// control riding the header's right edge, the rows under it.
pub fn section(label: &'static str, trailing: Option<AnyElement>, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(label),
                )
                .when_some(trailing, |d, trailing| d.child(trailing)),
        )
        .child(body)
}

/// The settings windows' text button, at the section header's scale
/// where every one of them rides: an icon leading its label; inert ones
/// dim and drop the click.
pub fn small_button(
    label: &'static str,
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control_hover()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(svg().path(icon).size(px(12.)).text_color(palette::text()))
        .child(label)
}

/// A flat icon-only button for table rows: the glyph alone at rest, a
/// soft pill behind it on hover, dimmed and inert like the text buttons.
pub fn icon_button(
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .p(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(svg().path(icon).size(px(14.)).text_color(palette::text()))
}

/// One scalar's slider: the shared slider chrome over a scrub strip,
/// applying live on click and drag, with the percent alongside.
pub fn slider<P: 'static>(
    scrub: &ScrubState,
    value: f32,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let readout = format!("{}%", (value * 100.0).round() as u32);
    slider_labeled(scrub, value, readout, apply, cx)
}

/// [`slider`] with the readout text exposed, for values whose natural
/// unit is not a percent (a pixel size, a count). `value` stays the 0 to
/// 1 strip fraction; the caller maps it to its range in `apply`.
pub fn slider_labeled<P: 'static>(
    scrub: &ScrubState,
    value: f32,
    readout: String,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let entity = cx.entity();
    let strip = div()
        .w(SLIDER_W)
        .h(tokens::CONTROL_H)
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let scrub = scrub.clone();
                let apply = apply.clone();
                move |this: &mut P, event: &MouseDownEvent, _, cx| {
                    scrub.begin();
                    if let Some(fraction) = scrub.fraction(event.position.x) {
                        apply(this, fraction, cx);
                    }
                    cx.notify();
                }
            }),
        )
        .child(
            canvas(
                {
                    let scrub = scrub.clone();
                    move |bounds, _, _| scrub.set_bounds(bounds)
                },
                {
                    let scrub = scrub.clone();
                    move |bounds, _, window, _| {
                        panel::paint_slider(value, false, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let entity = entity.clone();
                            let apply = apply.clone();
                            move |fraction, cx| {
                                entity.update(cx, |this, cx| apply(this, fraction, cx));
                            }
                        });
                    }
                },
            )
            .size_full(),
        );
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(strip)
        .child(
            div()
                .w(px(40.))
                .flex_none()
                .text_center()
                .text_color(palette::text_muted())
                .child(readout),
        )
}

/// One cell of a color grid: the swatch control with its role label
/// beside it. `marked` brightens the label, how the panel editor points
/// out the roles it overrides. `trailing` rides the cell's right edge,
/// where the panel editor hangs a role's reset button.
pub fn color_cell(
    control: AnyElement,
    label: &'static str,
    marked: bool,
    trailing: Option<AnyElement>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(control)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(if marked {
                    palette::text()
                } else {
                    palette::text_muted()
                })
                .child(label),
        )
        .when_some(trailing, |d, trailing| d.child(trailing))
}

/// The color grid's frame: each listing group under its header,
/// `columns` cells to a row, the last row padded so cells keep their
/// width. The cell for a role index is the caller's.
pub fn role_grid(columns: usize, mut cell: impl FnMut(usize) -> AnyElement) -> Div {
    let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
    let mut i = 0;
    while i < ROLES.len() {
        let group = ROLES[i].group;
        let end = ROLES[i..]
            .iter()
            .position(|role| role.group != group)
            .map(|n| i + n)
            .unwrap_or(ROLES.len());
        body = body.child(header(group));
        for row_start in (i..end).step_by(columns) {
            let mut row = div().flex().flex_row().gap(tokens::SPACE_SM);
            for j in row_start..row_start + columns {
                row = row.child(if j < end {
                    cell(j)
                } else {
                    div().flex_1().into_any_element()
                });
            }
            body = body.child(row);
        }
        i = end;
    }
    body
}
