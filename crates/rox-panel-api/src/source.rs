//! Which track a display panel describes: the playing one, or the app-wide
//! selection. Single-track panels like cover and lyrics store a
//! [`TrackSource`] in their per-view config and resolve it here.

use gpui::{App, Context, Div, Entity, Window};
use gpui_component::Side;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use serde::{Deserialize, Serialize};

use crate::panel::{self, AppState};
use rox_design::assets::icons;
use rox_library::cue::TrackKey;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackSource {
    #[default]
    Playing,
    Selected,
}

impl TrackSource {
    /// A key rather than a path, so one track of a cue rip resolves to that
    /// track and not its image.
    pub fn resolve(self, state: &AppState, cx: &App) -> Option<TrackKey> {
        match self {
            TrackSource::Playing => state.player.read(cx).now_playing().map(|now| now.key),
            TrackSource::Selected => {
                let id = state.selection.read(cx).tracks().first().copied()?;
                state.library.read(cx).keys_for(&[id]).ok()?.pop()
            }
        }
    }
}

/// Caches the selection side of [`TrackSource::resolve`], which is a
/// database query. Call [`ResolvedTrack::invalidate`] from the selection and
/// library subscriptions.
#[derive(Default)]
pub struct ResolvedTrack {
    selected: Option<Option<TrackKey>>,
}

impl ResolvedTrack {
    pub fn invalidate(&mut self) {
        self.selected = None;
    }

    pub fn get(&mut self, source: TrackSource, state: &AppState, cx: &App) -> Option<TrackKey> {
        match source {
            TrackSource::Playing => source.resolve(state, cx),
            TrackSource::Selected => self
                .selected
                .get_or_insert_with(|| source.resolve(state, cx))
                .clone(),
        }
    }
}

/// The track source as a "Track" flyout on a panel's dropdown menu.
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
        // Follow the panel so the tick swaps live while the menu is open.
        panel::follow_panel(&panel, cx);
        source_items(submenu.check_side(Side::Right), get, &panel, set)
    });
    menu.item(PopupMenuItem::submenu(
        rox_i18n::t!("source-track"),
        submenu,
    ))
}

fn source_items<P: 'static>(
    mut menu: PopupMenu,
    get: impl Fn(&P) -> TrackSource + Clone + 'static,
    panel: &Entity<P>,
    set: impl Fn(&mut P, TrackSource, &mut Context<P>) + Clone + 'static,
) -> PopupMenu {
    // Tick on the right, apart from each item's own icon.
    for (label, icon, source) in [
        (
            rox_i18n::t!("source-follow-playing"),
            icons::PLAY,
            TrackSource::Playing,
        ),
        (
            rox_i18n::t!("source-follow-selection"),
            icons::LIST_MUSIC,
            TrackSource::Selected,
        ),
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

pub fn source_row<P: 'static>(
    current: TrackSource,
    on_pick: impl Fn(&mut P, TrackSource, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::setting_row(
        rox_i18n::t!("source-track"),
        Some(rox_i18n::t!("source-track.description")),
        panel::choices_shared(
            &[
                (rox_i18n::t!("source-playing"), TrackSource::Playing),
                (rox_i18n::t!("source-selected"), TrackSource::Selected),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}
