//! The rating control shared by every surface that sets ratings: five stars,
//! or a 0-10 readout over twenty half-point steps when the rating style says
//! numeric. Clicking the value already set clears it.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use gpui::{App, Div, MouseButton, SharedString, Window, div, prelude::*, px, svg};

use rox_library::rating;

use rox_core::settings::{RatingStyle, rating_dots, rating_style};
use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// The hovered control and star, app-wide since only one control is under
/// the mouse. Statics because the control is a per-frame free function.
static HOVER_KEY: AtomicU64 = AtomicU64::new(0);
static HOVER_STAR: AtomicU8 = AtomicU8::new(0);

fn hover_star(key: u64) -> u8 {
    if HOVER_KEY.load(Ordering::Relaxed) == key {
        HOVER_STAR.load(Ordering::Relaxed)
    } else {
        0
    }
}

fn set_hover(key: u64, star: u8, window: &mut Window) {
    if HOVER_KEY.load(Ordering::Relaxed) == key && HOVER_STAR.load(Ordering::Relaxed) == star {
        return;
    }
    HOVER_KEY.store(key, Ordering::Relaxed);
    HOVER_STAR.store(star, Ordering::Relaxed);
    window.refresh();
}

/// Leaves the preview alone when another control's hover already took the key.
fn clear_hover(key: u64, window: &mut Window) {
    if HOVER_KEY.load(Ordering::Relaxed) != key {
        return;
    }
    HOVER_STAR.store(0, Ordering::Relaxed);
    window.refresh();
}

pub fn fmt(value: u8) -> SharedString {
    if value == 0 {
        "-".into()
    } else {
        rating::display(value).into()
    }
}

/// `key` names this control for the hover preview and must be stable and
/// unique to what it rates (the track id, an input's entity id).
pub fn control(
    key: u64,
    current: u8,
    set: impl Fn(u8, &mut Window, &mut App) + Clone + 'static,
) -> Div {
    let set = move |value: u8, window: &mut Window, cx: &mut App| {
        set(if value == current { 0 } else { value }, window, cx);
    };
    match rating_style() {
        RatingStyle::Stars => {
            // Round to the nearest whole star.
            let shown = (current + 10) / 20;
            let dots = rating_dots();
            let hovered = hover_star(key);
            let mut stars = div()
                .id(("rating-stars", key as usize))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(1.))
                .on_hover(move |hovering, window, _| {
                    if !hovering {
                        clear_hover(key, window);
                    }
                });
            for star in 1..=5u8 {
                let filled = star <= shown;
                let set = set.clone();
                let face = if star <= hovered {
                    svg()
                        .path(icons::STAR)
                        .size(px(14.))
                        .text_color(palette::accent())
                        .into_any_element()
                } else if filled || !dots {
                    svg()
                        .path(if filled {
                            icons::STAR_FILLED
                        } else {
                            icons::STAR
                        })
                        .size(px(14.))
                        .text_color(if filled {
                            palette::accent()
                        } else {
                            palette::text_faint()
                        })
                        .into_any_element()
                } else {
                    div()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(3.)).rounded_full().bg(palette::text_faint()))
                        .into_any_element()
                };
                stars = stars.child(
                    div()
                        .cursor_pointer()
                        .on_mouse_move(move |_, window, _| set_hover(key, star, window))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            set(star * 20, window, cx);
                        })
                        .child(face),
                );
            }
            // Wrap so callers still get a plain Div, not a Stateful one.
            div().flex().items_center().child(stars)
        }
        RatingStyle::Numeric => {
            let mut strip = div()
                .flex()
                .flex_row()
                .items_center()
                .flex_1()
                .cursor_pointer();
            for step in 1..=20u8 {
                let on = current >= step * 5;
                let set = set.clone();
                strip = strip.child(
                    div()
                        .flex_1()
                        .h(px(14.))
                        .flex()
                        .items_center()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            set(step * 5, window, cx);
                        })
                        .child(
                            div()
                                .h(px(3.))
                                .w_full()
                                .when(step == 1, |d| d.rounded_l_full())
                                .when(step == 20, |d| d.rounded_r_full())
                                .bg(if on {
                                    palette::accent()
                                } else {
                                    palette::bg_control()
                                }),
                        ),
                );
            }
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(
                    div()
                        .w(px(24.))
                        .flex_none()
                        .text_right()
                        .text_color(if current == 0 {
                            palette::text_faint()
                        } else {
                            palette::text_muted()
                        })
                        .child(fmt(current)),
                )
                .child(strip)
        }
    }
}
