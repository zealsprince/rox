//! The rating control: one clickable face over the library's 0-100
//! value, shared by every surface that sets ratings. Five stars, or a
//! 0-10 readout over twenty half-point steps when the app-level style
//! says numeric; clicking the value already held clears it. What a click
//! does with the value is the caller's - the library writes the catalog,
//! the tag editor arms a pending field.

use gpui::{div, prelude::*, px, svg, App, Div, MouseButton, SharedString, Window};

use rox_library::rating;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::settings::{rating_style, RatingStyle};

/// The readout form: the 0-10 display number, a dash while unrated.
pub fn fmt(value: u8) -> SharedString {
    if value == 0 {
        "-".into()
    } else {
        rating::display(value).into()
    }
}

/// The control over `current`, calling `set` with the clicked value - or
/// zero when the click lands on the value already held, the clear.
pub fn control(current: u8, set: impl Fn(u8, &mut Window, &mut App) + Clone + 'static) -> Div {
    let set = move |value: u8, window: &mut Window, cx: &mut App| {
        set(if value == current { 0 } else { value }, window, cx);
    };
    match rating_style() {
        RatingStyle::Stars => {
            // Filled to the nearest whole star, so a finer numeric score
            // still reads at a glance.
            let shown = (current + 10) / 20;
            let mut stars = div().flex().flex_row().items_center().gap(px(1.));
            for star in 1..=5u8 {
                let filled = star <= shown;
                let set = set.clone();
                stars = stars.child(
                    div()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            set(star * 20, window, cx);
                        })
                        .child(
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
                                }),
                        ),
                );
            }
            stars
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
