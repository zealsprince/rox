//! Shared scaffolding for the online-match windows (find metadata, find
//! lyrics, find cover art): the search-phase state they all cycle through
//! and the small confidence widgets the scored lists render.

use gpui::{div, prelude::*, px, Div, SharedString};

use crate::design::palette;

/// Where a match window is in its lookup: waiting on the network, holding a
/// ranked set of candidates, or showing why the search came back empty. The
/// candidate type differs per domain (metadata, lyrics, loaded covers), so
/// this is generic over it.
pub enum Phase<T> {
    Searching,
    Ready(Vec<T>),
    Failed(SharedString),
}

/// A one-word confidence tag beside a candidate's title, a quick read of how
/// far to trust the row before opening the preview.
pub fn confidence_badge(confidence: f32) -> Div {
    let pct = (confidence * 100.0).round() as u32;
    div()
        .flex_none()
        .text_xs()
        .text_color(if confidence >= 0.75 {
            palette::text_bright()
        } else {
            palette::text_muted()
        })
        .child(SharedString::from(format!("{pct}%")))
}

/// The confidence as a filled bar, so the list reads at a glance without
/// parsing the numbers.
pub fn confidence_bar(confidence: f32) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .rounded(px(2.))
        .bg(palette::bg_root())
        .child(
            div()
                .h_full()
                .rounded(px(2.))
                .w(gpui::relative(confidence.clamp(0.0, 1.0)))
                .bg(palette::accent()),
        )
}
