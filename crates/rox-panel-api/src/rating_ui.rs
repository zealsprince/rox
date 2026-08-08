//! The rating control: one clickable face over the library's 0-100
//! value, shared by every surface that sets ratings. Five stars, or a
//! 0-10 readout over twenty half-point steps when the app-level style
//! says numeric; clicking the value already held clears it. What a click
//! does with the value is the caller's - the library writes the catalog,
//! the tag editor arms a pending field.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use gpui::{div, prelude::*, px, svg, App, Div, MouseButton, SharedString, Window};

use rox_library::rating;

use rox_core::settings::{rating_dots, rating_style, RatingStyle};
use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// The star the pointer rests on, one pair app-wide: only one control
/// sits under the mouse at a time, and the key says which, so every
/// other control renders untouched. Star 0 is no preview. Statics
/// because the control is a free function rebuilt per frame with no
/// entity to hold state.
static HOVER_KEY: AtomicU64 = AtomicU64::new(0);
static HOVER_STAR: AtomicU8 = AtomicU8::new(0);

/// The previewed star for a control, 0 when the pointer is elsewhere.
fn hover_star(key: u64) -> u8 {
    if HOVER_KEY.load(Ordering::Relaxed) == key {
        HOVER_STAR.load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Note the pointer over a star and repaint; a move within the same
/// star costs nothing.
fn set_hover(key: u64, star: u8, window: &mut Window) {
    if HOVER_KEY.load(Ordering::Relaxed) == key && HOVER_STAR.load(Ordering::Relaxed) == star {
        return;
    }
    HOVER_KEY.store(key, Ordering::Relaxed);
    HOVER_STAR.store(star, Ordering::Relaxed);
    window.refresh();
}

/// Drop the preview when the pointer leaves this control; another
/// control's hover already replaced the key and keeps its own.
fn clear_hover(key: u64, window: &mut Window) {
    if HOVER_KEY.load(Ordering::Relaxed) != key {
        return;
    }
    HOVER_STAR.store(0, Ordering::Relaxed);
    window.refresh();
}

/// The readout form: the 0-10 display number, a dash while unrated.
pub fn fmt(value: u8) -> SharedString {
    if value == 0 {
        "-".into()
    } else {
        rating::display(value).into()
    }
}

/// The control over `current`, calling `set` with the clicked value - or
/// zero when the click lands on the value already held, the clear. `key`
/// names this control for the hover preview; callers pass something
/// stable and unique to what they rate (the track id, an input's entity
/// id), so hovering one control never lights another.
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
            // Filled to the nearest whole star, so a finer numeric score
            // still reads at a glance.
            let shown = (current + 10) / 20;
            let dots = rating_dots();
            // The pointer's preview: every star up to the hovered one
            // draws hollow in the accent, over filled and dotted rows
            // alike, so the click's landing value reads before it lands.
            let hovered = hover_star(key);
            // The id makes the row stateful, which is what carries the
            // hover-out that clears the preview.
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
                    // The unfilled slot as a quiet dot, the classic playlist
                    // look; centered in the star's box so the row of five
                    // keeps its width either way.
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
            // Wrapped so the stateful hover row stays inside while the
            // callers keep styling the plain Div they always got.
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
