//! Shared scaffolding for the online-match windows (find metadata, find
//! lyrics, find cover art): the search-phase state they all cycle through,
//! the small confidence widgets the scored lists render, the centered
//! "note" line their empty states show, and the open-or-focus dance the
//! editor and matcher windows all run over a keyed window registry.

use gpui::{div, prelude::*, px, App, Div, Global, SharedString, WindowHandle};
use gpui_component::Root;

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

/// A quiet centered line where a search window's list or grid would sit -
/// its empty, searching, and failed states share this.
pub fn note(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::text_faint())
        .child(text.into())
}

/// A `Global` holding the live windows for one editor or matcher kind,
/// keyed so a second request for the same subject focuses the open window
/// instead of stacking a twin. Each kind keeps its own newtype (so the six
/// registries never cross-talk); the key is whatever tells one window from
/// another - sorted track ids, a path, or a path plus the opening editor's
/// id where the window binds to a specific editor.
pub trait WindowRegistry: Global + Default {
    type Key: PartialEq;
    fn entries(&mut self) -> &mut Vec<(Self::Key, WindowHandle<Root>)>;
}

/// Open a window for `key`, or bring the one already on that key to the
/// front. The probe doubles as a sweep: a window whose handle no longer
/// updates has been closed, so it drops out of the registry here. `build`
/// runs only when no live window matches; it opens the OS window and hands
/// back its handle, which registers under `key`.
pub fn open_or_focus<R: WindowRegistry>(
    key: R::Key,
    build: impl FnOnce(&mut App) -> WindowHandle<Root>,
    cx: &mut App,
) {
    let entries = std::mem::take(cx.default_global::<R>().entries());
    // Closed windows fall out of the list as a side effect of the probe.
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
