//! Shared scaffolding for the online-match windows: the search phase, the
//! confidence widgets, and the open-or-focus window registry.

use gpui::{App, Div, Global, SharedString, WindowHandle, div, prelude::*, px};
use gpui_component::Root;

use rox_design::palette;

pub enum Phase<T> {
    Searching,
    Ready(Vec<T>),
    Failed(SharedString),
}

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
        .child(SharedString::from(rox_i18n::format::format_percent(
            pct as f64,
        )))
}

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

pub fn note(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::text_faint())
        .child(text.into())
}

/// The live windows for one editor or matcher kind, keyed so a repeat
/// request focuses the open window instead of stacking a twin.
pub trait WindowRegistry: Global + Default {
    type Key: PartialEq;
    fn entries(&mut self) -> &mut Vec<(Self::Key, WindowHandle<Root>)>;
}

/// Open a window for `key`, or focus the open one. Closed windows drop out
/// of the registry as a side effect.
pub fn open_or_focus<R: WindowRegistry>(
    key: R::Key,
    build: impl FnOnce(&mut App) -> WindowHandle<Root>,
    cx: &mut App,
) {
    let entries = std::mem::take(cx.default_global::<R>().entries());
    let mut alive = Vec::with_capacity(entries.len() + 1);
    let mut focused = false;
    for (entry_key, handle) in entries {
        let matches = entry_key == key;
        if handle
            .update(cx, |_, window, _| {
                if matches {
                    window.activate_window();
                }
            })
            .is_ok()
        {
            focused |= matches;
            alive.push((entry_key, handle));
        }
    }
    if !focused {
        alive.push((key, build(cx)));
    }
    *cx.default_global::<R>().entries() = alive;
}
