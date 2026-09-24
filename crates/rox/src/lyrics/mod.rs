//! Lyrics windows: the sheet editor and the online matcher, plus the save
//! signal both ring.

pub mod edit;
pub mod matcher;

use gpui::{App, Global, WeakEntity};

use rox_library::lyrics::Subject;
use rox_panels::lyrics::LyricsPanel;

/// Every live lyrics panel. Lyrics aren't in the projection, so this
/// registry is the only way a save reaches the panels.
#[derive(Default)]
struct Watchers(Vec<WeakEntity<LyricsPanel>>);

impl Global for Watchers {}

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

pub fn saved(subject: &Subject, cx: &mut App) {
    poke(cx, |panel, cx| panel.reload(subject, cx));
}

/// Show the editor's unsaved draft for `subject` in every panel, or drop it
/// with None.
pub fn preview(subject: &Subject, text: Option<&str>, cx: &mut App) {
    poke(cx, |panel, cx| panel.set_preview(subject, text, cx));
}

fn poke(cx: &mut App, mut f: impl FnMut(&mut LyricsPanel, &mut gpui::Context<LyricsPanel>)) {
    let watchers = std::mem::take(&mut cx.default_global::<Watchers>().0);
    let mut alive = Vec::with_capacity(watchers.len());
    for panel in watchers {
        if panel.update(cx, &mut f).is_ok() {
            alive.push(panel);
        }
    }
    // A panel that registered while the pokes ran keeps its spot.
    let watchers = cx.default_global::<Watchers>();
    alive.append(&mut watchers.0);
    watchers.0 = alive;
}
