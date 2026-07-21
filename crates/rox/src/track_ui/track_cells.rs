//! The per-track rating and favourite controls shared by the library table
//! and the playlists tree. Both write straight into the catalog through the
//! same [`Library`](crate::panels::library::Library) methods, so a star or a
//! heart set in one surface shows in the other. Each cell hides its empty
//! affordance until the row is hovered, so a column of them stays quiet;
//! that reveal keys off the row wearing [`ROW_GROUP`].

use gpui::{div, prelude::*, px, svg, Div, MouseButton};

use crate::assets::icons;
use crate::design::palette;
use crate::panel::AppState;

/// The hover group a track row wears so its rating and favourite cells can
/// reveal on hover. Both panels tag their track rows with this.
pub const ROW_GROUP: &str = "track-row";

/// The rating control over a track's value, writing a click straight into
/// the catalog by id. An unrated track keeps the cell invisible until its
/// row is hovered; the control stops the mouse-down itself, so rating never
/// reselects or plays the row.
pub fn rating(state: AppState, id: i64, value: u8) -> Div {
    crate::rating_ui::control(value, move |rating, _, cx| {
        state
            .library
            .update(cx, |library, cx| library.rate(id, rating, cx));
    })
    // Fill the cell height so the control's own items_center lands the stars
    // on the row centerline.
    .h_full()
    .when(value == 0, |d| {
        d.opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
    })
}

/// A heart that fills when the track is in the favourites playlist and
/// toggles on click. An unfavourited track keeps the outline hidden until
/// the row is hovered; the click stops its own mouse-down so it never
/// reselects the row.
pub fn favourite(state: AppState, id: i64, on: bool) -> Div {
    div()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            svg()
                .path(if on { icons::HEART_FILLED } else { icons::HEART })
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
