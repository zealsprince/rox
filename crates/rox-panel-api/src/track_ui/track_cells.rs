//! The per-track rating and favourite controls shared by the library table
//! and the track-list panels. Each cell hides its empty affordance until the
//! row, tagged with [`ROW_GROUP`], is hovered.

use gpui::{Div, MouseButton, div, prelude::*, px, svg};

use crate::panel::AppState;
use rox_design::assets::icons;
use rox_design::palette;

/// Every track list tags its rows with this hover group.
pub const ROW_GROUP: &str = "track-row";

/// An unrated track hides the control until row hover, unless the
/// unrated-dots setting is on.
pub fn rating(state: AppState, id: i64, value: u8) -> Div {
    crate::rating_ui::control(id as u64, value, move |rating, _, cx| {
        state
            .library
            .update(cx, |library, cx| library.rate(id, rating, cx));
    })
    // Full height so the stars sit on the row centerline.
    .h_full()
    .when(value == 0 && !rox_core::settings::rating_dots(), |d| {
        d.opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
    })
}

/// An unfavourited track hides the outline until row hover.
pub fn favourite(state: AppState, id: i64, on: bool) -> Div {
    div()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            svg()
                .path(if on {
                    icons::HEART_FILLED
                } else {
                    icons::HEART
                })
                .size(px(15.))
                .text_color(if on {
                    palette::accent()
                } else {
                    palette::text_faint()
                }),
        )
        .when(!on, |d| {
            d.opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cx.stop_propagation();
            state
                .library
                .update(cx, |library, cx| library.set_favourites(&[id], !on, cx));
        })
}
