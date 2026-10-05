//! Playing a song the library only knows by name, an Unknown row
//! ([`rox_library::unknown`]). A plugin searches for it, and the result it
//! plays takes over the row's listens and heart. History and the Favourites
//! playlist both list these rows and share this.

use gpui::{App, Context, Entity, SharedString, Task, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};

use rox_library::cue::TrackKey;
use rox_services::{plugins, unknown};

use crate::panel::{self, AppState};

pub(crate) fn is_unknown(source: &str) -> bool {
    source == rox_library::unknown::SOURCE
}

/// The plugin a search goes to: the Play From pick, then the panel's
/// remembered one, then the only one running. Err is the line to show
/// instead.
pub(crate) fn source_for(
    picked: Option<String>,
    remembered: Option<&str>,
) -> Result<(String, String), SharedString> {
    let sources = plugins::searchable_sources();

    // A remembered plugin that's since stopped doesn't count.
    let remembered = remembered
        .filter(|source| sources.iter().any(|(id, _)| id == source))
        .map(str::to_string);
    let only = (sources.len() == 1).then(|| sources[0].0.clone());

    let Some(source) = picked.or(remembered).or(only) else {
        return Err(match sources.is_empty() {
            true => rox_i18n::t!("history-no-plugins"),
            false => rox_i18n::t!("history-pick-plugin"),
        });
    };

    let label = sources
        .iter()
        .find(|(id, _)| *id == source)
        .map_or_else(|| source.clone(), |(_, label)| label.clone());

    Ok((source, label))
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

/// What the search came back with. A hit plays at once, and the row's
/// listens and heart follow the song onto the plugin's row, so the next
/// click plays it straight away. None means there's nothing left to say.
pub(crate) fn found(
    state: &AppState,
    unknown_id: i64,
    found: Result<Option<TrackKey>, String>,
    label: String,
    title: String,
    cx: &mut App,
) -> Option<SharedString> {
    match found {
        Ok(Some(key)) => {
            state
                .player
                .update(cx, |player, cx| player.play_at(vec![key.clone()], 0, cx));

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

/// The plugins an Unknown row can be searched on, with the panel's
/// remembered pick checked.
pub(crate) fn play_from_menu<P: 'static>(
    menu: PopupMenu,
    panel: Entity<P>,
    remembered: impl Fn(&P) -> Option<String> + Clone + 'static,
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
            let remembered = remembered.clone();
            let pick = pick.clone();
            submenu = submenu.item(panel::check_row(
                label,
                None,
                move |this: &P| remembered(this).as_ref() == Some(&checked),
                move |this, cx| pick(this, source.clone(), cx),
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
