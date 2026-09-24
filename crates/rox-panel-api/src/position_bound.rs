//! The one question every mark, cue and loop asks first: is there a
//! position to hang this off?
//!
//! A live stream has no timeline. Its clock counts listening time, so a
//! mark placed at "two minutes in" points nowhere on the next listen. One
//! shared answer and reason, so strips, menus and keys lock out together.

use gpui::{App, SharedString, div, prelude::*};
use gpui_component::Icon;
use gpui_component::menu::PopupMenuItem;
use rox_panel_kit::Tip;

use crate::panel::AppState;

/// False only while the playing entry is a live stream. The idle player
/// counts as allowed; its commands have their own reasons to do nothing.
pub fn allowed(state: &AppState, cx: &App) -> bool {
    !state
        .player
        .read(cx)
        .now_playing()
        .is_some_and(|now| now.live)
}

pub fn reason() -> SharedString {
    rox_i18n::t!("position-bound-streaming")
}

/// A position-moving menu row, greyed with the reason on hover rather than
/// dropped. An element item because the stock row can't carry a tooltip.
pub fn locked_item(label: SharedString, icon: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| {
        Tip::keyed(label.clone(), reason()).apply(div().child(label.clone()))
    })
    .icon(Icon::default().path(icon))
    .disabled(true)
}
