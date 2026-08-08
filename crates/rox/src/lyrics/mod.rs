//! Lyrics windows: the editor for hand-writing or fixing a sheet and the
//! online matcher that pulls synced lyrics from the providers, plus the
//! app-wide save signal both of them ring.

pub mod edit;
pub mod matcher;

use std::path::Path;

use gpui::{App, Global, WeakEntity};

use rox_panels::lyrics::LyricsPanel;

/// Every live lyrics panel. Lyrics do not ride the library projection, so
/// a sheet written by the editor or the matcher has no other way to reach
/// the panels showing that track - including the duplicates in other tabs
/// and windows, which is why this is a registry and not the opening
/// panel's handle. Weak: a closed panel drops out on the next sweep.
#[derive(Default)]
struct Watchers(Vec<WeakEntity<LyricsPanel>>);

impl Global for Watchers {}

/// Register a panel to hear saves, sweeping the handles that have died
/// since the last call so a long session does not grow the list.
pub fn watch(panel: WeakEntity<LyricsPanel>, cx: &mut App) {
    let watchers = cx.default_global::<Watchers>();
    watchers.0.retain(|w| w.upgrade().is_some());
    if watchers
        .0
        .iter()
        .any(|w| w.entity_id() == panel.entity_id())
    {
        return;
    }
    watchers.0.push(panel);
}

/// A sheet for `path` landed on disk: every live panel drops its cache for
/// that track and re-reads on the next render.
pub fn saved(path: &Path, cx: &mut App) {
    let watchers = std::mem::take(&mut cx.default_global::<Watchers>().0);
    let mut alive = Vec::with_capacity(watchers.len());
    for panel in watchers {
        if panel.update(cx, |panel, cx| panel.reload(path, cx)).is_ok() {
            alive.push(panel);
        }
    }
    // A panel that registered while the pokes ran keeps its spot.
    let watchers = cx.default_global::<Watchers>();
    alive.append(&mut watchers.0);
    watchers.0 = alive;
}
