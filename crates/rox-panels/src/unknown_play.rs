//! Playing a song the library only knows by name, an Unknown row
//! ([`rox_library::unknown`]). A plugin searches for it, and the result it
//! plays takes over the row's listens and heart. History and the Favourites
//! playlist both list these rows and share this. The Unknown rows around
//! the one played go into the queue as they are, and the player searches
//! for each as it comes up ([`rox_services::unknown_resolver`]).

use gpui::{
    App, Context, DismissEvent, Entity, Focusable, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, SharedString, Styled, Subscription, Task, Window, anchored, deferred, div, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};

use rox_core::settings::{self, Settings};
use rox_library::cue::TrackKey;
use rox_services::{plugins, unknown, unknown_resolver};

use crate::continuation::Scope;
use crate::panel::{self, AppState};

pub(crate) fn is_unknown(source: &str) -> bool {
    source == rox_library::unknown::SOURCE
}

/// Where a search on an Unknown row goes.
pub(crate) enum Lookup {
    /// The (source, label) to search.
    Ready(String, String),
    /// Several plugins run and none is set, so the picker asks.
    Ask,
    /// No running plugin can search.
    Missing,
}

/// The plugin a search goes to: the Play From pick, then the Plugins page's
/// default, then the only one running.
pub(crate) fn lookup(picked: Option<String>) -> Lookup {
    let sources = plugins::searchable_sources();
    let default = settings::unknown_plugin();
    let Some(source) =
        unknown_resolver::source_for(picked.as_deref(), default.as_deref(), &sources)
    else {
        return match sources.is_empty() {
            true => Lookup::Missing,
            false => Lookup::Ask,
        };
    };

    let label = sources
        .iter()
        .find(|(id, _)| *id == source)
        .map_or_else(|| source.clone(), |(_, label)| label.clone());

    Lookup::Ready(source, label)
}

/// The plugin a play here hands the player for the Unknown rows it queues.
pub(crate) fn session_source() -> Option<String> {
    match lookup(None) {
        Lookup::Ready(source, _) => Some(source),
        _ => None,
    }
}

/// A Play From pick becomes the default everywhere, the same one the Plugins
/// page sets.
pub(crate) fn remember(source: &str) {
    let source = source.to_string();
    settings::set_unknown_plugin(Some(source.clone()));
    Settings::update(move |s| s.unknown_plugin = Some(source));
}

pub(crate) fn finding_line(label: &str, title: &str) -> SharedString {
    rox_i18n::t!(
        "history-finding",
        source = label.to_string(),
        title = title.to_string()
    )
}

pub(crate) fn find(
    state: &AppState,
    source: &str,
    artist: String,
    title: String,
    cx: &mut App,
) -> Task<Result<Option<TrackKey>, String>> {
    plugins::find_track(state.library.clone(), source, artist, title, cx)
}

/// The rows an Unknown row was clicked among, taken when the search starts.
/// They resolve when it lands, so a row deleted meanwhile drops out.
pub(crate) struct Spot {
    /// In play order, the clicked row at `at`.
    pub ids: Vec<i64>,
    pub at: usize,
    /// The order continuation follows past `ids`, Unknown rows included,
    /// since the player queues them for a search too. None follows the ids
    /// that resolve.
    pub view: Option<Vec<i64>>,
}

/// The keys before and after `ids[at]`, each side resolved on its own. A
/// deleted row before the click drops out of `before`, so `before.len()` is
/// the clicked row's start with nothing to shift.
pub(crate) fn around(
    ids: &[i64],
    at: usize,
    resolve: impl Fn(&[i64]) -> Vec<TrackKey>,
) -> (Vec<TrackKey>, Vec<TrackKey>) {
    let at = at.min(ids.len());
    let before = resolve(&ids[..at]);
    let after = ids.get(at + 1..).map_or_else(Vec::new, &resolve);

    (before, after)
}

/// Plays `key` between its neighbours, then hands continuation the view
/// and the player the plugin its Unknown rows are searched on.
pub(crate) fn play_between(
    state: &AppState,
    before: Vec<TrackKey>,
    key: TrackKey,
    after: Vec<TrackKey>,
    view: Option<Vec<i64>>,
    unknown_source: Option<String>,
    cx: &mut App,
) {
    let start = before.len();
    let mut keys = before;
    keys.push(key);
    keys.extend(after);

    state.player.update(cx, |player, cx| {
        player.play_at(keys, start, cx);
        // After the play: starting a session resets the scope and the pick.
        if let Some(view) = view {
            player.set_scope(Scope::View(view.into()));
        }
        player.set_unknown_source(unknown_source);
    });
}

/// What the search on `pick`, a (source, label) pair, came back with. A hit
/// plays at once in the clicked row's place, and the row's listens and heart
/// follow the song onto the plugin's row, so the next click plays it straight
/// away. None means there's nothing left to say.
pub(crate) fn found(
    state: &AppState,
    unknown_id: i64,
    spot: Spot,
    found: Result<Option<TrackKey>, String>,
    pick: (String, String),
    title: String,
    cx: &mut App,
) -> Option<SharedString> {
    let (source, label) = pick;
    match found {
        Ok(Some(key)) => {
            let (before, after, view) = {
                let library = state.library.read(cx);
                let resolve = |ids: &[i64]| library.queue_keys_for(ids).unwrap_or_default();
                let (before, after) = around(&spot.ids, spot.at, resolve);

                let view = spot.view.unwrap_or_else(|| {
                    spot.ids
                        .iter()
                        .copied()
                        .filter(|&id| !resolve(&[id]).is_empty())
                        .collect()
                });
                (before, after, view)
            };
            play_between(
                state,
                before,
                key.clone(),
                after,
                Some(view),
                Some(source),
                cx,
            );

            let adopted = unknown::adopt(state.library.clone(), unknown_id, key, cx);
            cx.spawn(async move |_| {
                if let Err(e) = adopted.await {
                    log::warn!("moving an unknown row onto its plugin track: {e}");
                }
            })
            .detach();
            None
        }

        Ok(None) => Some(rox_i18n::t!(
            "history-not-found",
            source = label,
            title = title
        )),

        Err(reason) => Some(rox_i18n::t!(
            "history-find-failed",
            source = label,
            reason = reason
        )),
    }
}

/// The plugins an Unknown row can be searched on, with the default checked.
pub(crate) fn play_from_menu<P: 'static>(
    menu: PopupMenu,
    panel: Entity<P>,
    pick: impl Fn(&mut P, String, &mut Context<P>) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let sources = plugins::searchable_sources();
    if sources.is_empty() {
        return menu.item(PopupMenuItem::new(rox_i18n::t!("history-no-plugins")).disabled(true));
    }

    let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
        panel::follow_panel(&panel, cx);
        for (source, label) in sources {
            let checked = source.clone();
            let pick = pick.clone();
            submenu = submenu.item(panel::check_row(
                label,
                None,
                move |_: &P| settings::unknown_plugin().as_ref() == Some(&checked),
                move |this, cx| {
                    remember(&source);
                    pick(this, source.clone(), cx);
                },
                &panel,
            ));
        }
        submenu
    });
    menu.item(PopupMenuItem::submenu(
        rox_i18n::t!("history-play-from"),
        submenu,
    ))
}

/// The Play From list a play on an Unknown row opens at the pointer when
/// several plugins run and none is set.
#[derive(Default)]
pub(crate) struct Picker(Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>);

impl Picker {
    pub(crate) fn ask<P: 'static>(
        slot: fn(&mut P) -> &mut Picker,
        pick: impl Fn(&mut P, String, &mut Context<P>) + Clone + 'static,
        this: &mut P,
        window: &mut Window,
        cx: &mut Context<P>,
    ) {
        let sources = plugins::searchable_sources();
        let panel = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            menu = menu.label(rox_i18n::t!("history-play-from"));
            for (source, label) in sources {
                let panel = panel.clone();
                let pick = pick.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                    remember(&source);
                    panel
                        .update(cx, |this, cx| pick(this, source.clone(), cx))
                        .ok();
                }));
            }
            menu
        });
        menu.focus_handle(cx).focus(window);

        let dismissed = cx.subscribe(&menu, move |this: &mut P, _, _: &DismissEvent, cx| {
            slot(this).0 = None;
            cx.notify();
        });
        *slot(this) = Picker(Some((window.mouse_position(), menu, dismissed)));
        cx.notify();
    }

    /// The overlay the panel's root carries while the picker is open. An
    /// occluding layer swallows the dismissing click, the way the dock's
    /// context menu does.
    pub(crate) fn overlay(&self, window: &Window) -> Option<impl IntoElement + use<>> {
        let (position, menu, _) = self.0.as_ref()?;
        let size = window.bounds().size;

        Some(
            deferred(
                anchored().child(
                    div().w(size.width).h(size.height).occlude().child(
                        anchored()
                            .position(*position)
                            .snap_to_window_with_margin(px(8.))
                            .child(menu.clone()),
                    ),
                ),
            )
            .with_priority(1),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rox_library::cue::TrackKey;

    use super::around;

    fn key(id: i64) -> TrackKey {
        TrackKey::from(PathBuf::from(format!("/m/{id}.flac")))
    }

    /// A deleted row resolves to nothing, the way `queue_keys_for` drops
    /// it, and an Unknown row resolves to its own key.
    #[test]
    fn found_key_takes_the_clicked_slot_among_unknown_neighbours() {
        let (unknown_a, deleted, local1, unknown_clicked, unknown_b, local2) = (1, 2, 3, 4, 5, 6);
        let ids = [
            unknown_a,
            deleted,
            local1,
            unknown_clicked,
            unknown_b,
            local2,
        ];
        let resolve = |ids: &[i64]| {
            ids.iter()
                .filter(|&&id| id != deleted)
                .map(|&id| key(id))
                .collect()
        };

        let (before, after) = around(&ids, 3, resolve);
        assert!(before == vec![key(unknown_a), key(local1)]);
        assert!(after == vec![key(unknown_b), key(local2)]);
        assert!(before.len() == 2, "the found key starts the queue at 2");
    }

    #[test]
    fn around_the_ends_has_one_side_empty() {
        let resolve = |ids: &[i64]| ids.iter().map(|&id| key(id)).collect();

        let (before, after) = around(&[1, 2, 3], 0, resolve);
        assert!(before.is_empty());
        assert!(after == vec![key(2), key(3)]);

        let (before, after) = around(&[1, 2, 3], 2, resolve);
        assert!(before == vec![key(1), key(2)]);
        assert!(after.is_empty());
    }
}
