//! Which track a display panel describes: the playing one, or the app-wide
//! selection ([`crate::selection`]). Panels that show a single track (cover
//! art today, richer track views later) carry a [`TrackSource`] in their
//! per-view config and resolve it here at render time; the setting row is
//! shared so the knob reads the same in every customize window.

use std::path::PathBuf;

use gpui::{App, Context, Div, Entity, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Side;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::panel::{self, AppState};

/// The two places a displayed track can come from.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackSource {
    #[default]
    Playing,
    Selected,
}

impl TrackSource {
    /// The track the source currently points at: the playing file, or the
    /// first track of the selection resolved back to its path.
    pub fn resolve(self, state: &AppState, cx: &App) -> Option<PathBuf> {
        match self {
            TrackSource::Playing => state.player.read(cx).now_playing().map(|now| now.path),
            TrackSource::Selected => {
                let id = state.selection.read(cx).tracks().first().copied()?;
                state.library.read(cx).paths_for(&[id]).ok()?.pop()
            }
        }
    }
}

/// A per-view cache over [`TrackSource::resolve`], for panels that render
/// every frame while a session runs: the selection side is a database
/// query, so it only re-runs after [`ResolvedTrack::invalidate`] - call
/// that from the selection and library subscriptions. The playing side
/// stays uncached, it reads shared atomics.
#[derive(Default)]
pub struct ResolvedTrack {
    selected: Option<Option<PathBuf>>,
}

impl ResolvedTrack {
    /// The selection or the catalog changed; the next get re-resolves.
    pub fn invalidate(&mut self) {
        self.selected = None;
    }

    pub fn get(&mut self, source: TrackSource, state: &AppState, cx: &App) -> Option<PathBuf> {
        match source {
            TrackSource::Playing => source.resolve(state, cx),
            TrackSource::Selected => self
                .selected
                .get_or_insert_with(|| source.resolve(state, cx))
                .clone(),
        }
    }
}

/// The track source as a "Track" flyout on a panel's dropdown menu: one
/// checked entry per source, the same knob as [`source_row`], so the follow
/// mode reads the same everywhere.
pub fn source_flyout<P: 'static>(
    menu: PopupMenu,
    get: impl Fn(&P) -> TrackSource + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, TrackSource, &mut Context<P>) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let panel = panel.clone();
    let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
        // The flyout follows the panel so the picked row's tick swaps live
        // instead of sitting stale until the menu is reopened.
        panel::follow_panel(&panel, cx);
        source_items(submenu.check_side(Side::Right), get, &panel, set)
    });
    menu.item(PopupMenuItem::submenu("Track", submenu))
}

/// The checked source rows the [`source_flyout`] lists.
fn source_items<P: 'static>(
    mut menu: PopupMenu,
    get: impl Fn(&P) -> TrackSource + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, TrackSource, &mut Context<P>) + Clone + 'static,
) -> PopupMenu {
    // Each source item carries its own icon (Play, List Music), so the tick
    // sits on the right where it stands apart from the icon.
    for (label, icon, source) in [
        ("Follow Playing", icons::PLAY, TrackSource::Playing),
        ("Follow Selection", icons::LIST_MUSIC, TrackSource::Selected),
    ] {
        let get = get.clone();
        let set = set.clone();
        menu = menu.item(panel::check_row(
            label,
            Some(icon),
            move |this: &P| get(this) == source,
            move |this, cx| set(this, source, cx),
            panel,
        ));
    }
    menu
}

/// The source setting row for a panel's customize window.
pub fn source_row<P: 'static>(
    current: TrackSource,
    on_pick: impl Fn(&mut P, TrackSource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::setting_row(
        "Track",
        Some("Follow what is playing, or what is selected in the library"),
        panel::choices(
            &[
                ("Playing", TrackSource::Playing),
                ("Selected", TrackSource::Selected),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}
