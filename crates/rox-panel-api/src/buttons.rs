//! The declared table of player state a custom button's look can follow:
//! every entry names a readable piece of state, its finite list of cases, and
//! a stock icon and colour role per case. The button editor generates one row
//! per case, so a user picks from lists and never writes an expression.
//!
//! Free of [`gpui`] so the table tests without a context. Colours are
//! [`palette::ROLES`] names, so a re-themed palette carries through to buttons
//! built under the old one. The `player.` ids read the player and the `app.`
//! ids read a global or another app service, and both are persisted in saved
//! layouts forever.
//!
//! The stock look is seed data that makes a new button a working clone of the
//! native control. Nothing in `rox-panels` renders through here. Left out:
//! A-B's breathing dot, the play button's accent fill, and mute's level split,
//! which no case can say, and the rating, which no command sets.

use rox_core::continuation;
use rox_core::settings::{self, GainModeSetting, ShuffleMode};
use rox_design::assets::icons;
use rox_design::palette;
use rox_playback::{broadcast, icy};
use rox_services::catalog::Library;
use rox_services::discord_presence::DiscordPresence;
use rox_services::lastfm::Scrobbler;
use rox_services::player::{self, AbState, LoopMode, Player};

use crate::marks::Marks;

/// The live state a reader may look at, borrowed for one draw.
pub struct Live<'a> {
    pub player: &'a Player,
    pub library: &'a Library,
    pub scrobbler: &'a Scrobbler,
    pub discord: &'a DiscordPresence,
    /// The playing track's marks. Resolving them costs queries, so the host
    /// only does it when a placed button [`needs_marks`]; None otherwise and
    /// while nothing plays.
    pub marks: Option<Marks>,
    pub shell: Shell,
}

/// What only the shell can say about the window a button draws in. The host
/// fills it in, since this crate can't name a workspace.
#[derive(Clone, Copy, Default)]
pub struct Shell {
    pub mini: bool,
}

pub struct StateSpec {
    /// Stable forever: it is what a saved layout holds. "player.repeat".
    pub id: &'static str,
    pub label_key: &'static str,
    /// In editor order. Exhaustive: `read` always returns one of these ids.
    pub cases: &'static [StateCase],
    /// Prefilled when the user picks the state. Empty when there's no one
    /// obvious command.
    pub action: &'static str,
    pub read: fn(&Live) -> &'static str,
}

pub struct StateCase {
    pub id: &'static str,
    pub label_key: &'static str,
    /// An `icons::CATALOG` path.
    pub icon: &'static str,
    /// A `palette::ROLES` name.
    pub color: &'static str,
}

/// In the order the state picker lists them.
pub const STATES: &[StateSpec] = &[
    StateSpec {
        id: "player.playback",
        label_key: "button-state-playback",
        action: "toggle_playback",
        read: read_playback,
        // The glyph is the action: playing shows the pause, like the native
        // play button.
        cases: &[
            StateCase {
                id: "playing",
                label_key: "button-state-playback-playing",
                icon: icons::PAUSE,
                color: "text",
            },
            StateCase {
                id: "paused",
                label_key: "button-state-playback-paused",
                icon: icons::PLAY,
                color: "text",
            },
        ],
    },
    StateSpec {
        id: "player.favourite",
        label_key: "button-state-favourite",
        action: "toggle_favourite",
        read: read_favourite,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-favourite-on",
                icon: icons::HEART_FILLED,
                color: "accent",
            },
            StateCase {
                id: "half",
                label_key: "button-state-favourite-half",
                icon: icons::HEART_HALF,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-favourite-off",
                icon: icons::HEART,
                color: "text",
            },
            StateCase {
                id: "none",
                label_key: "button-state-favourite-none",
                icon: icons::HEART,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.repeat",
        label_key: "button-state-repeat",
        action: "cycle_loop",
        read: read_repeat,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-repeat-off",
                icon: icons::REPEAT,
                color: "text_faint",
            },
            StateCase {
                id: "all",
                label_key: "button-state-repeat-all",
                icon: icons::REPEAT,
                color: "accent",
            },
            StateCase {
                id: "one",
                label_key: "button-state-repeat-one",
                icon: icons::REPEAT_1,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "player.shuffle",
        label_key: "button-state-shuffle",
        action: "cycle_shuffle_mode",
        read: read_shuffle,
        // The native strip takes this glyph but its colour from shuffle_on, so
        // both seeds here take plain text.
        cases: &[
            StateCase {
                id: "random",
                label_key: "button-state-shuffle-random",
                icon: icons::SHUFFLE,
                color: "text",
            },
            StateCase {
                id: "similar",
                label_key: "button-state-shuffle-similar",
                icon: icons::RADIO,
                color: "text",
            },
        ],
    },
    StateSpec {
        id: "player.shuffle_on",
        label_key: "button-state-shuffling",
        action: "toggle_shuffle",
        read: read_shuffle_on,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-shuffling-on",
                icon: icons::SHUFFLE,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-shuffling-off",
                icon: icons::LIST_ORDERED,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.mute",
        label_key: "button-state-mute",
        action: "toggle_mute",
        read: read_mute,
        cases: &[
            StateCase {
                id: "muted",
                label_key: "button-state-mute-muted",
                icon: icons::VOLUME_X,
                color: "text_faint",
            },
            StateCase {
                id: "unmuted",
                label_key: "button-state-mute-unmuted",
                icon: icons::VOLUME_2,
                color: "text",
            },
        ],
    },
    StateSpec {
        id: "player.stop_after",
        label_key: "button-state-stop-after",
        action: "toggle_stop_after",
        read: read_stop_after,
        // Armed takes the solid square: the dashed one is what an unconfigured
        // button wears.
        cases: &[
            StateCase {
                id: "armed",
                label_key: "button-state-stop-after-armed",
                icon: icons::STOP,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-stop-after-off",
                icon: icons::SQUARE_DASHED,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.ab_repeat",
        label_key: "button-state-ab-repeat",
        action: "ab_repeat",
        read: read_ab_repeat,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-ab-repeat-off",
                icon: icons::MOVE_HORIZONTAL,
                color: "text_faint",
            },
            StateCase {
                id: "a-set",
                label_key: "button-state-ab-repeat-a-set",
                icon: icons::FLAG,
                color: "text",
            },
            StateCase {
                id: "looping",
                label_key: "button-state-ab-repeat-looping",
                icon: icons::ITERATION_CCW,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "player.continuation",
        label_key: "button-state-continuation",
        action: "toggle_continuation",
        read: read_continuation,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-continuation-off",
                icon: icons::INFINITY,
                color: "text_faint",
            },
            StateCase {
                id: "continue",
                label_key: "button-state-continuation-continue",
                icon: icons::INFINITY,
                color: "accent",
            },
            StateCase {
                id: "weighted",
                label_key: "button-state-continuation-weighted",
                icon: icons::INFINITY,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "player.stop",
        label_key: "button-state-stop",
        action: "stop_playback",
        read: read_stop,
        cases: &[
            StateCase {
                id: "active",
                label_key: "button-state-stop-active",
                icon: icons::STOP,
                color: "text",
            },
            StateCase {
                id: "idle",
                label_key: "button-state-stop-idle",
                icon: icons::STOP,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.crossfade",
        label_key: "button-state-crossfade",
        action: "toggle_crossfade",
        read: read_crossfade,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-crossfade-off",
                icon: icons::BLEND,
                color: "text_faint",
            },
            StateCase {
                id: "on",
                label_key: "button-state-crossfade-on",
                icon: icons::BLEND,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "player.crossfade_albums",
        label_key: "button-state-crossfade-albums",
        action: "toggle_crossfade_albums",
        read: read_crossfade_albums,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-crossfade-albums-off",
                icon: icons::BLEND,
                color: "text_faint",
            },
            StateCase {
                id: "on",
                label_key: "button-state-crossfade-albums-on",
                icon: icons::BLEND,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "player.sleep",
        label_key: "button-state-sleep",
        // Arming carries a length a command can't hold, so the command is the
        // cancel.
        action: "sleep_off",
        read: read_sleep,
        cases: &[
            StateCase {
                id: "armed",
                label_key: "button-state-sleep-armed",
                icon: icons::BED,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-sleep-off",
                icon: icons::BED,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.eq",
        label_key: "button-state-eq",
        action: "toggle_eq",
        read: read_eq,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-eq-on",
                icon: icons::SLIDERS,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-eq-off",
                icon: icons::SLIDERS,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.exclusive_output",
        label_key: "button-state-exclusive-output",
        action: "toggle_exclusive_output",
        read: read_exclusive_output,
        // What was asked for, not what the device granted, same as the
        // settings page.
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-exclusive-output-on",
                icon: icons::HEADPHONES,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-exclusive-output-off",
                icon: icons::HEADPHONES,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "player.replaygain",
        label_key: "button-state-replaygain",
        action: "cycle_replaygain_mode",
        read: read_replaygain,
        cases: &[
            StateCase {
                id: "off",
                label_key: "button-state-replaygain-off",
                icon: icons::GAUGE,
                color: "text_faint",
            },
            StateCase {
                id: "track",
                label_key: "button-state-replaygain-track",
                icon: icons::GAUGE,
                color: "accent",
            },
            StateCase {
                id: "album",
                label_key: "button-state-replaygain-album",
                icon: icons::GAUGE,
                color: "accent",
            },
        ],
    },
    StateSpec {
        id: "app.theme",
        label_key: "button-state-theme",
        action: "toggle_theme",
        read: read_theme,
        // The side in effect, unlike the theme toggle panel, which draws the
        // side a click goes to.
        cases: &[
            StateCase {
                id: "dark",
                label_key: "button-state-theme-dark",
                icon: icons::MOON,
                color: "text",
            },
            StateCase {
                id: "light",
                label_key: "button-state-theme-light",
                icon: icons::SUN,
                color: "text",
            },
        ],
    },
    StateSpec {
        id: "app.design_mode",
        label_key: "button-state-design-mode",
        action: "toggle_design_mode",
        read: read_design_mode,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-design-mode-on",
                icon: icons::LAYOUT_DASHBOARD,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-design-mode-off",
                icon: icons::LAYOUT_DASHBOARD,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.resize_lock",
        label_key: "button-state-resize-lock",
        action: "toggle_resize_lock",
        read: read_resize_lock,
        cases: &[
            StateCase {
                id: "locked",
                label_key: "button-state-resize-lock-locked",
                icon: icons::LOCK,
                color: "accent",
            },
            StateCase {
                id: "unlocked",
                label_key: "button-state-resize-lock-unlocked",
                icon: icons::LOCK_OPEN,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.mini",
        label_key: "button-state-mini",
        action: "toggle_mini",
        read: read_mini,
        // The glyph is the way the click goes, like the native mini toggle.
        cases: &[
            StateCase {
                id: "mini",
                label_key: "button-state-mini-mini",
                icon: icons::MAXIMIZE,
                color: "text",
            },
            StateCase {
                id: "primary",
                label_key: "button-state-mini-primary",
                icon: icons::MINIMIZE,
                color: "text",
            },
        ],
    },
    StateSpec {
        id: "app.menubar",
        label_key: "button-state-menubar",
        action: "toggle_menubar",
        read: read_menubar,
        // Named for what the menubar does: the flag is hide_menubar, and a
        // button reading "hidden: on" would be a riddle.
        cases: &[
            StateCase {
                id: "shown",
                label_key: "button-state-menubar-shown",
                icon: icons::EYE,
                color: "accent",
            },
            StateCase {
                id: "hidden",
                label_key: "button-state-menubar-hidden",
                icon: icons::EYE_OFF,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.decorations",
        label_key: "button-state-decorations",
        action: "toggle_decorations",
        read: read_decorations,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-decorations-on",
                icon: icons::APP_WINDOW,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-decorations-off",
                icon: icons::APP_WINDOW,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.art_theming",
        label_key: "button-state-art-theming",
        action: "toggle_art_theming",
        read: read_art_theming,
        // The disc, since the palette glyph belongs to the theme.
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-art-theming-on",
                icon: icons::DISC,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-art-theming-off",
                icon: icons::DISC,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.quit_to_tray",
        label_key: "button-state-quit-to-tray",
        action: "toggle_quit_to_tray",
        read: read_quit_to_tray,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-quit-to-tray-on",
                icon: icons::MINIMIZE,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-quit-to-tray-off",
                icon: icons::MINIMIZE,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.seams",
        label_key: "button-state-seams",
        action: "toggle_seams",
        read: read_seams,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-seams-on",
                icon: icons::COLUMNS_2,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-seams-off",
                icon: icons::COLUMNS_2,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.readings",
        label_key: "button-state-readings",
        action: "toggle_readings",
        read: read_readings,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-readings-on",
                icon: icons::A_LARGE_SMALL,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-readings-off",
                icon: icons::A_LARGE_SMALL,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.scrobbling",
        label_key: "button-state-scrobbling",
        action: "toggle_scrobbling",
        read: read_scrobbling,
        // The one switch every destination shares, whether or not an account is
        // connected, same as the settings page.
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-scrobbling-on",
                icon: icons::CLOUD_UPLOAD,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-scrobbling-off",
                icon: icons::CLOUD_OFF,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.discord",
        label_key: "button-state-discord",
        action: "toggle_discord",
        read: read_discord,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-discord-on",
                icon: icons::GAMEPAD,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-discord-off",
                icon: icons::GAMEPAD,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.broadcast",
        label_key: "button-state-broadcast",
        action: "toggle_broadcast",
        read: read_broadcast,
        // What was asked for, not whether the server took the stream.
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-broadcast-on",
                icon: icons::RADIO_TOWER,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-broadcast-off",
                icon: icons::RADIO_TOWER,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.capture",
        label_key: "button-state-capture",
        action: "toggle_capture",
        read: read_capture,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-capture-on",
                icon: icons::CIRCLE_DOT,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-capture-off",
                icon: icons::CIRCLE_DOT,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.watch_folders",
        label_key: "button-state-watch-folders",
        action: "toggle_watch_folders",
        read: read_watch_folders,
        // What was asked for: past the platform's watch ceiling the watcher
        // never arms, same as the settings page keeps the preference.
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-watch-folders-on",
                icon: icons::FOLDER,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-watch-folders-off",
                icon: icons::FOLDER,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.post_shader",
        label_key: "button-state-post-shader",
        action: "toggle_post_shader",
        read: read_post_shader,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-post-shader-on",
                icon: icons::BLEND,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-post-shader-off",
                icon: icons::BLEND,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.backdrop_shader",
        label_key: "button-state-backdrop-shader",
        action: "toggle_backdrop_shader",
        read: read_backdrop_shader,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-backdrop-shader-on",
                icon: icons::LAYERS,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-backdrop-shader-off",
                icon: icons::LAYERS,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.backdrop_all_windows",
        label_key: "button-state-backdrop-all-windows",
        action: "toggle_backdrop_all_windows",
        read: read_backdrop_all_windows,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-backdrop-all-windows-on",
                icon: icons::APP_WINDOW,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-backdrop-all-windows-off",
                icon: icons::APP_WINDOW,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.milkdrop_backdrop",
        label_key: "button-state-milkdrop-backdrop",
        action: "toggle_milkdrop_backdrop",
        read: read_milkdrop_backdrop,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-milkdrop-backdrop-on",
                icon: icons::AUDIO_LINES,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-milkdrop-backdrop-off",
                icon: icons::AUDIO_LINES,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.milkdrop_hard_cuts",
        label_key: "button-state-milkdrop-hard-cuts",
        action: "toggle_milkdrop_hard_cuts",
        read: read_milkdrop_hard_cuts,
        cases: &[
            StateCase {
                id: "on",
                label_key: "button-state-milkdrop-hard-cuts-on",
                icon: icons::ACTIVITY,
                color: "accent",
            },
            StateCase {
                id: "off",
                label_key: "button-state-milkdrop-hard-cuts-off",
                icon: icons::ACTIVITY,
                color: "text_faint",
            },
        ],
    },
    StateSpec {
        id: "app.milkdrop_lock",
        label_key: "button-state-milkdrop-lock",
        action: "toggle_milkdrop_lock",
        read: read_milkdrop_lock,
        cases: &[
            StateCase {
                id: "locked",
                label_key: "button-state-milkdrop-lock-locked",
                icon: icons::PIN,
                color: "accent",
            },
            StateCase {
                id: "unlocked",
                label_key: "button-state-milkdrop-lock-unlocked",
                icon: icons::PIN_OFF,
                color: "text_faint",
            },
        ],
    },
];

/// None for an unknown id, so a layout written by a newer build draws its
/// fallback instead of misfiring.
pub fn read_state(id: &str, live: &Live) -> Option<&'static str> {
    let spec = spec(id)?;

    Some((spec.read)(live))
}

/// Split out so the unknown-id case tests without a live player.
fn spec(id: &str) -> Option<&'static StateSpec> {
    STATES.iter().find(|spec| spec.id == id)
}

/// The states that read [`Live::marks`].
const MARKED: &[&str] = &["player.favourite"];

pub fn needs_marks(id: &str) -> bool {
    MARKED.contains(&id)
}

fn read_playback(live: &Live) -> &'static str {
    if live.player.is_playing() {
        "playing"
    } else {
        "paused"
    }
}

fn read_favourite(live: &Live) -> &'static str {
    match live.marks {
        Some(marks @ Marks { id: Some(_), .. }) if marks.half() => "half",
        Some(Marks {
            id: Some(_),
            favourite: true,
            ..
        }) => "on",
        Some(Marks { id: Some(_), .. }) => "off",
        _ => "none",
    }
}

fn read_repeat(live: &Live) -> &'static str {
    match live.player.loop_mode() {
        LoopMode::Off => "off",
        LoopMode::All => "all",
        LoopMode::One => "one",
    }
}

fn read_shuffle(live: &Live) -> &'static str {
    match live.player.shuffle_mode() {
        ShuffleMode::Random => "random",
        ShuffleMode::Similar => "similar",
    }
}

fn read_shuffle_on(live: &Live) -> &'static str {
    if live.player.shuffle() { "on" } else { "off" }
}

fn read_mute(live: &Live) -> &'static str {
    if live.player.muted() {
        "muted"
    } else {
        "unmuted"
    }
}

fn read_stop_after(live: &Live) -> &'static str {
    if live.player.stop_after() {
        "armed"
    } else {
        "off"
    }
}

fn read_ab_repeat(live: &Live) -> &'static str {
    match live.player.ab_state() {
        AbState::Off => "off",
        AbState::ASet(_) => "a-set",
        AbState::Looping(..) => "looping",
    }
}

fn read_continuation(live: &Live) -> &'static str {
    match live.player.continuation_mode() {
        continuation::Mode::Off => "off",
        continuation::Mode::Continue => "continue",
        continuation::Mode::Weighted => "weighted",
    }
}

fn read_stop(live: &Live) -> &'static str {
    if live.player.is_active() {
        "active"
    } else {
        "idle"
    }
}

fn read_crossfade(live: &Live) -> &'static str {
    if live.player.crossfade_secs() > 0.0 {
        "on"
    } else {
        "off"
    }
}

fn read_crossfade_albums(live: &Live) -> &'static str {
    if live.player.crossfade_albums() {
        "on"
    } else {
        "off"
    }
}

fn read_sleep(live: &Live) -> &'static str {
    if live.player.sleep_remaining().is_some() {
        "armed"
    } else {
        "off"
    }
}

fn read_replaygain(live: &Live) -> &'static str {
    match live.player.replay_gain().mode {
        GainModeSetting::Off => "off",
        GainModeSetting::Track => "track",
        GainModeSetting::Album => "album",
    }
}

fn read_exclusive_output(live: &Live) -> &'static str {
    if live.player.exclusive_output() {
        "on"
    } else {
        "off"
    }
}

/// The EQ reads a free getter over atomics, like the `app.` readers below.
fn read_eq(_live: &Live) -> &'static str {
    if player::eq_enabled() { "on" } else { "off" }
}

fn read_theme(_live: &Live) -> &'static str {
    match palette::mode() {
        palette::Mode::Dark => "dark",
        palette::Mode::Light => "light",
    }
}

fn read_design_mode(_live: &Live) -> &'static str {
    if settings::design_mode() { "on" } else { "off" }
}

fn read_resize_lock(_live: &Live) -> &'static str {
    if settings::resize_lock() {
        "locked"
    } else {
        "unlocked"
    }
}

fn read_mini(live: &Live) -> &'static str {
    if live.shell.mini { "mini" } else { "primary" }
}

fn read_menubar(_live: &Live) -> &'static str {
    if settings::hide_menubar() {
        "hidden"
    } else {
        "shown"
    }
}

fn read_decorations(_live: &Live) -> &'static str {
    if settings::os_decorations() {
        "on"
    } else {
        "off"
    }
}

fn read_art_theming(_live: &Live) -> &'static str {
    if palette::art_theming() { "on" } else { "off" }
}

fn read_quit_to_tray(_live: &Live) -> &'static str {
    if settings::quit_to_tray() {
        "on"
    } else {
        "off"
    }
}

fn read_seams(_live: &Live) -> &'static str {
    if settings::seams() { "on" } else { "off" }
}

fn read_readings(_live: &Live) -> &'static str {
    if settings::show_readings() {
        "on"
    } else {
        "off"
    }
}

fn read_scrobbling(live: &Live) -> &'static str {
    if live.scrobbler.scrobbling() {
        "on"
    } else {
        "off"
    }
}

fn read_discord(live: &Live) -> &'static str {
    if live.discord.enabled() { "on" } else { "off" }
}

fn read_broadcast(_live: &Live) -> &'static str {
    if broadcast::enabled() { "on" } else { "off" }
}

fn read_capture(_live: &Live) -> &'static str {
    if icy::capturing() { "on" } else { "off" }
}

fn read_watch_folders(live: &Live) -> &'static str {
    if live.library.watch_on() { "on" } else { "off" }
}

fn read_post_shader(_live: &Live) -> &'static str {
    if settings::post_shader_on() {
        "on"
    } else {
        "off"
    }
}

fn read_backdrop_shader(_live: &Live) -> &'static str {
    if settings::backdrop_shader_on() {
        "on"
    } else {
        "off"
    }
}

fn read_backdrop_all_windows(_live: &Live) -> &'static str {
    if palette::backdrop_all_windows() {
        "on"
    } else {
        "off"
    }
}

fn read_milkdrop_hard_cuts(_live: &Live) -> &'static str {
    if settings::backdrop_visual().hard_cuts {
        "on"
    } else {
        "off"
    }
}

fn read_milkdrop_backdrop(_live: &Live) -> &'static str {
    if settings::backdrop_visual().enabled {
        "on"
    } else {
        "off"
    }
}

fn read_milkdrop_lock(_live: &Live) -> &'static str {
    if settings::backdrop_visual().locked {
        "locked"
    } else {
        "unlocked"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rox_design::palette;

    /// A duplicate case id shadows whichever row the user edits second.
    #[test]
    fn every_state_case_is_reachable() {
        for spec in STATES {
            assert!(!spec.cases.is_empty(), "{} has no cases", spec.id);

            for (i, case) in spec.cases.iter().enumerate() {
                let dupe = spec.cases[..i].iter().any(|other| other.id == case.id);
                assert!(!dupe, "{} repeats case {}", spec.id, case.id);
            }
        }
    }

    /// A seed outside `CATALOG` is an icon the user can never pick again once
    /// they've edited it away.
    #[test]
    fn stock_icons_are_in_the_catalog() {
        for spec in STATES {
            for case in spec.cases {
                assert!(
                    icons::CATALOG.contains(&case.icon),
                    "{}/{} seeds {}, which is not in the icon catalog",
                    spec.id,
                    case.id,
                    case.icon
                );
            }
        }
    }

    #[test]
    fn stock_colours_name_real_roles() {
        for spec in STATES {
            for case in spec.cases {
                assert!(
                    palette::ROLES.iter().any(|role| role.name == case.color),
                    "{}/{} seeds colour {}, which is not a palette role",
                    spec.id,
                    case.id,
                    case.color
                );
            }
        }
    }

    #[test]
    fn state_ids_are_unique() {
        for (i, spec) in STATES.iter().enumerate() {
            let dupe = STATES[..i].iter().any(|other| other.id == spec.id);
            assert!(!dupe, "{} is listed twice", spec.id);
        }
    }

    /// A typo here leaves a favourite button reading "none" forever.
    #[test]
    fn marked_states_exist() {
        for id in MARKED {
            assert!(spec(id).is_some(), "{id} is marked but not a state");
        }
    }

    #[test]
    fn read_state_refuses_an_unknown_id() {
        assert!(spec("player.nonsense").is_none());
        assert!(spec("").is_none());
        assert!(spec("player.repeat").is_some());
    }
}
