//! Every chord rox binds, in one list, for the Keymap page to draw and the
//! settings file to override. The file only holds what someone changed, so a
//! default that moves in a later build reaches everyone who left it alone.
//!
//! gpui offers add and clear but no remove, so a rebind rebuilds the whole
//! keymap in layers, later winning at equal depth: the widget library's
//! bindings (snapshotted by [`init`]), then the windows' fixed chords
//! ([`fixed`]), then the commands.
//!
//! This is the only module that calls `bind_keys`. A window that wants a
//! chord joins [`fixed`]. Anything bound behind its back is carried forward
//! on the next rebuild with a warning.

use std::collections::BTreeMap;
use std::sync::{LazyLock, PoisonError, RwLock};

use gpui::{Action, App, Global, KeyBinding, Keystroke, Window};

use rox_core::settings::Settings;

use rox_dock::{NextTab, OpenPanelSettings, PrevTab, ToggleZoom};
use rox_panel_api::actions::{
    SeekBackward, SeekForward, TogglePlayback, TypeAheadNext, TypeAheadPrev,
};
use rox_panels::lyrics::StampLine;

use crate::workspace::{
    AbClear, AbRepeat, AbortScan, AddBookmark, AddNamedBookmark, AnalyzeTempo, BuildAcoustic,
    ClearQueue, ClosePanelAction, CloseWindow, Cue, CueClear, CueNext, CuePrev, CycleLoop,
    CycleReplayGainMode, CycleShuffleMode, DecreaseFontSize, FillSortNames, FindDuplicates,
    FlattenEq, FocusSearch, ImportWorkspace, IncreaseFontSize, MeasureReplayGain, NewEmptyWindow,
    NewWindow, NextBookmark, NextTrack, OpenAbout, OpenChat, OpenConsole, OpenDiscussions,
    OpenEqualizer, OpenGoTo, OpenHealth, OpenPowerSearch, OpenPresetPicker, OpenQuickPlay,
    OpenSettings, OpenSignals, OpenStats, OpenTasks, OpenWelcome, PlayRandom, PlaySimilar,
    PrevBookmark, PreviousTrack, Quit, ReportIssue, RescanLibrary, ResetFontSize, RomanizeLibrary,
    SaveLayout, SaveWorkspace, SleepOff, StepBackward, StepForward, StopPlayback, TagGenres,
    ToggleArtTheming, ToggleContinuation, ToggleCrossfade, ToggleCrossfadeAlbums,
    ToggleDecorations, ToggleDesignMode, ToggleEq, ToggleExclusiveOutput, ToggleFavourite,
    ToggleMenubar, ToggleMini, ToggleMute, TogglePostShader, ToggleQuitToTray, ToggleReadings,
    ToggleResizeLock, ToggleSeams, ToggleShuffle, ToggleStopAfter, ToggleTheme, VolumeDown,
    VolumeUp,
};

/// Workspace-wide except while the search box, a type-ahead phrase, the
/// menubar, or a tabbed-to control holds focus, so space and arrows go to
/// them. Bindings beat key listeners, so the exclusion hands the keys back.
/// A modified chord widens to [`WORKSPACE`]; see [`Command::scope`].
const PLAYBACK: Option<&str> =
    Some("Workspace && !SearchInput && !TypeAhead && !MenuNav && !FocusedControl");

/// [`PLAYBACK`] minus panels whose own left and right mean something (a tile
/// wall, a folder tree). Only the seek and step pairs use it.
const SEEK: Option<&str> =
    Some("Workspace && !SearchInput && !TypeAhead && !MenuNav && !PanelNav && !FocusedControl");

const WORKSPACE: Option<&str> = Some("Workspace");

const LYRICS: Option<&str> = Some("LyricsEdit");

/// Only present while a panel holds a phrase, so tab walks panels otherwise.
const TYPE_AHEAD: Option<&str> = Some(rox_panel_kit::TYPE_AHEAD_CYCLE_CONTEXT);

/// The Keymap page draws one section per group, in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Playback,
    Library,
    Windows,
    Browsing,
    View,
    Editing,
}

impl Group {
    pub const ALL: &'static [Group] = &[
        Group::Playback,
        Group::Library,
        Group::Windows,
        Group::Browsing,
        Group::View,
        Group::Editing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Group::Playback => rox_i18n::t_static("keymap-group-playback"),
            Group::Library => rox_i18n::t_static("keymap-group-library"),
            Group::Windows => rox_i18n::t_static("keymap-group-windows"),
            Group::Browsing => rox_i18n::t_static("keymap-group-browsing"),
            Group::View => rox_i18n::t_static("keymap-group-view"),
            Group::Editing => rox_i18n::t_static("keymap-group-editing"),
        }
    }

    pub fn icon(self) -> &'static str {
        use rox_design::assets::icons;
        match self {
            Group::Playback => icons::PLAY,
            Group::Library => icons::MUSIC,
            Group::Windows => icons::APP_WINDOW,
            Group::Browsing => icons::SEARCH,
            Group::View => icons::EYE,
            Group::Editing => icons::PENCIL,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Reach {
    /// Handled by an app-level `cx.on_action`, so a click fires it whatever holds focus.
    Global,
    /// Handled on a panel, so firing it needs a target. Not offered to buttons.
    Panel,
}

pub struct Command {
    /// The settings key. Never rename it: that silently resets everyone's rebinds.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub group: Group,
    /// `None` is everywhere, including the settings window and popped-out panels.
    pub context: Option<&'static str>,
    /// gpui syntax. Several are aliases, not a sequence; empty ships unbound.
    pub defaults: &'static [&'static str],
    pub reach: Reach,
    /// gpui's registered name ("rox::TogglePlayback"), for [`dispatch`].
    action_name: &'static str,
    /// The action type can't be stored as data, so it's baked into this fn.
    build: fn(&str, Option<&'static str>) -> KeyBinding,
}

impl Command {
    /// None for a chord that doesn't parse, so one bad hand edit doesn't block the rest.
    fn binding(&self, chord: &str) -> Option<KeyBinding> {
        parses(chord).then(|| (self.build)(chord, self.scope(chord)))
    }

    /// A modified chord on a narrowed scope widens to the whole workspace: the
    /// exclusions only exist for keys a focused input would eat.
    fn scope(&self, chord: &str) -> Option<&'static str> {
        if narrowed(self.context) && modified(chord) {
            WORKSPACE
        } else {
            self.context
        }
    }
}

/// Dispatch `id` at the window like a keybinding, for custom buttons. False
/// for an unknown id or a non-global reach, which also guards a saved layout
/// naming a command the picker no longer offers.
pub fn dispatch(id: &str, window: &mut Window, cx: &mut App) -> bool {
    let Some(command) = COMMANDS.iter().find(|command| command.id == id) else {
        return false;
    };

    if command.reach != Reach::Global {
        return false;
    }

    let Ok(action) = cx.build_action(command.action_name, None) else {
        return false;
    };

    window.dispatch_action(action, cx);
    true
}

pub fn global_commands() -> impl Iterator<Item = &'static Command> {
    COMMANDS
        .iter()
        .filter(|command| command.reach == Reach::Global)
}

fn narrowed(scope: Option<&'static str>) -> bool {
    scope == PLAYBACK || scope == SEEK
}

/// Shift doesn't count: shift-letter is typing. Only the first keystroke
/// matters, since that's what a focused input would eat.
fn modified(chord: &str) -> bool {
    chord
        .split_whitespace()
        .next()
        .and_then(|key| Keystroke::parse(key).ok())
        .is_some_and(|key| {
            let m = key.modifiers;
            m.control || m.alt || m.platform || m.function
        })
}

macro_rules! command {
    // Must come first, or the shorter arm matches and swallows the reach.
    ($id:literal, $label:expr, $group:expr, $ctx:expr, $keys:expr, $action:expr, $desc:expr, $reach:expr) => {
        Command {
            id: $id,
            label: $label,
            description: $desc,
            group: $group,
            context: $ctx,
            defaults: $keys,
            reach: $reach,
            action_name: Action::name(&$action),
            build: |keys, ctx| KeyBinding::new(keys, $action, ctx),
        }
    };

    ($id:literal, $label:expr, $group:expr, $ctx:expr, $keys:expr, $action:expr, $desc:expr) => {
        command!(
            $id,
            $label,
            $group,
            $ctx,
            $keys,
            $action,
            $desc,
            Reach::Global
        )
    };
}

// macOS puts app-level chords on Cmd; everywhere else they're on Ctrl.
#[cfg(target_os = "macos")]
mod defaults {
    pub const SETTINGS: &[&str] = &["cmd-,", "ctrl-i"];
    pub const PANEL_SETTINGS: &[&str] = &["cmd-shift-,"];
    pub const STATS: &[&str] = &["cmd-shift-s"];
    pub const HEALTH: &[&str] = &["cmd-shift-h"];
    pub const POWER_SEARCH: &[&str] = &["cmd-shift-f"];
    pub const QUICK_PLAY: &[&str] = &["cmd-p", "cmd-f"];
    pub const FOCUS_SEARCH: &[&str] = &["cmd-l"];
    pub const ZOOM_IN: &[&str] = &["cmd-=", "cmd-+"];
    pub const ZOOM_OUT: &[&str] = &["cmd--"];
    pub const ZOOM_RESET: &[&str] = &["cmd-0"];
    pub const POST_SHADER: &[&str] = &["cmd-shift-x"];
    pub const CLOSE_WINDOW: &[&str] = &["cmd-w"];
    pub const QUIT: &[&str] = &["cmd-q"];
    pub const NEW_WINDOW: &[&str] = &["cmd-n"];
    pub const TASKS: &[&str] = &["cmd-j"];
    pub const EQUALIZER: &[&str] = &["cmd-e"];
    pub const PRESET_PICKER: &[&str] = &["cmd-shift-v"];
    pub const NEXT_TRACK: &[&str] = &["cmd-right"];
    pub const PREVIOUS_TRACK: &[&str] = &["cmd-left"];
    pub const STOP: &[&str] = &["cmd-."];
    pub const GO_TO_TIME: &[&str] = &["cmd-g"];
    pub const AB_REPEAT: &[&str] = &["l", "cmd-shift-l"];
    pub const BOOKMARK: &[&str] = &["m"];
    pub const BOOKMARK_NAMED: &[&str] = &["shift-m"];
    pub const CUE: &[&str] = &["n"];
    pub const PREV_BOOKMARK: &[&str] = &["ctrl-shift-left"];
    pub const NEXT_BOOKMARK: &[&str] = &["ctrl-shift-right"];
    pub const PLAY_RANDOM: &[&str] = &["cmd-r"];
    pub const MUTE: &[&str] = &["cmd-shift-m"];
    pub const DESIGN_MODE: &[&str] = &["cmd-shift-d"];
    pub const THEME: &[&str] = &["cmd-shift-t"];
}

#[cfg(not(target_os = "macos"))]
mod defaults {
    pub const SETTINGS: &[&str] = &["ctrl-,", "ctrl-i"];
    pub const PANEL_SETTINGS: &[&str] = &["ctrl-<"];
    pub const STATS: &[&str] = &["ctrl-shift-s"];
    pub const HEALTH: &[&str] = &["ctrl-shift-h"];
    pub const POWER_SEARCH: &[&str] = &["ctrl-shift-f"];
    pub const QUICK_PLAY: &[&str] = &["ctrl-p", "ctrl-f"];
    pub const FOCUS_SEARCH: &[&str] = &["ctrl-l"];
    pub const ZOOM_IN: &[&str] = &["ctrl-=", "ctrl-+"];
    pub const ZOOM_OUT: &[&str] = &["ctrl--"];
    pub const ZOOM_RESET: &[&str] = &["ctrl-0"];
    pub const POST_SHADER: &[&str] = &["ctrl-shift-x"];
    pub const CLOSE_WINDOW: &[&str] = &["ctrl-w"];
    pub const QUIT: &[&str] = &["alt-f4"];
    pub const NEW_WINDOW: &[&str] = &["ctrl-n"];
    pub const TASKS: &[&str] = &["ctrl-j"];
    pub const EQUALIZER: &[&str] = &["ctrl-e"];
    pub const PRESET_PICKER: &[&str] = &["ctrl-shift-v"];
    pub const NEXT_TRACK: &[&str] = &["ctrl-right"];
    pub const PREVIOUS_TRACK: &[&str] = &["ctrl-left"];
    pub const STOP: &[&str] = &["ctrl-."];
    pub const GO_TO_TIME: &[&str] = &["ctrl-g"];
    pub const AB_REPEAT: &[&str] = &["l", "ctrl-shift-l"];
    pub const BOOKMARK: &[&str] = &["m"];
    pub const BOOKMARK_NAMED: &[&str] = &["shift-m"];
    pub const CUE: &[&str] = &["n"];
    pub const PREV_BOOKMARK: &[&str] = &["ctrl-shift-left"];
    pub const NEXT_BOOKMARK: &[&str] = &["ctrl-shift-right"];
    pub const PLAY_RANDOM: &[&str] = &["ctrl-r"];
    pub const MUTE: &[&str] = &["ctrl-shift-m"];
    pub const DESIGN_MODE: &[&str] = &["ctrl-shift-d"];
    pub const THEME: &[&str] = &["ctrl-shift-t"];
}

/// Page order within each group. A `LazyLock` because the labels resolve
/// through the locale bundles; iterate with `.iter()`, since deref coercion
/// doesn't reach `IntoIterator`.
pub static COMMANDS: LazyLock<Vec<Command>> = LazyLock::new(|| {
    vec![
        command!(
            "toggle_playback",
            rox_i18n::t_static("keymap-toggle-playback"),
            Group::Playback,
            PLAYBACK,
            &["space"],
            TogglePlayback,
            rox_i18n::t_static("keymap-toggle-playback.description")
        ),
        command!(
            "seek_backward",
            rox_i18n::t_static("keymap-seek-backward"),
            Group::Playback,
            SEEK,
            &["left"],
            SeekBackward,
            rox_i18n::t_static("keymap-seek-backward.description")
        ),
        command!(
            "seek_forward",
            rox_i18n::t_static("keymap-seek-forward"),
            Group::Playback,
            SEEK,
            &["right"],
            SeekForward,
            rox_i18n::t_static("keymap-seek-forward.description")
        ),
        // Comma and dot, the video editor's frame keys. On the seek scope so the
        // pair stays one control if rebound onto arrows.
        command!(
            "step_backward",
            rox_i18n::t_static("keymap-step-backward"),
            Group::Playback,
            SEEK,
            &[","],
            StepBackward,
            rox_i18n::t_static("keymap-step-backward.description")
        ),
        command!(
            "step_forward",
            rox_i18n::t_static("keymap-step-forward"),
            Group::Playback,
            SEEK,
            &["."],
            StepForward,
            rox_i18n::t_static("keymap-step-forward.description")
        ),
        command!(
            "go_to_time",
            rox_i18n::t_static("keymap-go-to-time"),
            Group::Playback,
            WORKSPACE,
            defaults::GO_TO_TIME,
            OpenGoTo,
            rox_i18n::t_static("keymap-go-to-time.description")
        ),
        command!(
            "stop_playback",
            rox_i18n::t_static("keymap-stop-playback"),
            Group::Playback,
            WORKSPACE,
            defaults::STOP,
            StopPlayback,
            rox_i18n::t_static("keymap-stop-playback.description")
        ),
        // Bare l is mpv's ab-loop key, with the same three-press cycle.
        command!(
            "ab_repeat",
            rox_i18n::t_static("keymap-ab-repeat"),
            Group::Playback,
            PLAYBACK,
            defaults::AB_REPEAT,
            AbRepeat,
            rox_i18n::t_static("keymap-ab-repeat.description")
        ),
        command!(
            "ab_clear",
            rox_i18n::t_static("keymap-ab-clear"),
            Group::Playback,
            WORKSPACE,
            &[],
            AbClear,
            rox_i18n::t_static("keymap-ab-clear.description")
        ),
        command!(
            "bookmark",
            rox_i18n::t_static("keymap-bookmark"),
            Group::Playback,
            PLAYBACK,
            defaults::BOOKMARK,
            AddBookmark,
            rox_i18n::t_static("keymap-bookmark.description")
        ),
        command!(
            "bookmark_named",
            rox_i18n::t_static("keymap-bookmark-named"),
            Group::Playback,
            PLAYBACK,
            defaults::BOOKMARK_NAMED,
            AddNamedBookmark,
            rox_i18n::t_static("keymap-bookmark-named.description")
        ),
        command!(
            "prev_bookmark",
            rox_i18n::t_static("keymap-prev-bookmark"),
            Group::Playback,
            WORKSPACE,
            defaults::PREV_BOOKMARK,
            PrevBookmark,
            rox_i18n::t_static("keymap-prev-bookmark.description")
        ),
        command!(
            "next_bookmark",
            rox_i18n::t_static("keymap-next-bookmark"),
            Group::Playback,
            WORKSPACE,
            defaults::NEXT_BOOKMARK,
            NextBookmark,
            rox_i18n::t_static("keymap-next-bookmark.description")
        ),
        command!(
            "cue",
            rox_i18n::t_static("keymap-cue"),
            Group::Playback,
            PLAYBACK,
            defaults::CUE,
            Cue,
            rox_i18n::t_static("keymap-cue.description")
        ),
        command!(
            "cue_prev",
            rox_i18n::t_static("keymap-cue-prev"),
            Group::Playback,
            WORKSPACE,
            &[],
            CuePrev,
            rox_i18n::t_static("keymap-cue-prev.description")
        ),
        command!(
            "cue_next",
            rox_i18n::t_static("keymap-cue-next"),
            Group::Playback,
            WORKSPACE,
            &[],
            CueNext,
            rox_i18n::t_static("keymap-cue-next.description")
        ),
        // Unbound on purpose: it throws away work.
        command!(
            "cue_clear",
            rox_i18n::t_static("keymap-cue-clear"),
            Group::Playback,
            WORKSPACE,
            &[],
            CueClear,
            rox_i18n::t_static("keymap-cue-clear.description")
        ),
        command!(
            "next_track",
            rox_i18n::t_static("keymap-next-track"),
            Group::Playback,
            WORKSPACE,
            defaults::NEXT_TRACK,
            NextTrack,
            rox_i18n::t_static("keymap-next-track.description")
        ),
        command!(
            "previous_track",
            rox_i18n::t_static("keymap-previous-track"),
            Group::Playback,
            WORKSPACE,
            defaults::PREVIOUS_TRACK,
            PreviousTrack,
            rox_i18n::t_static("keymap-previous-track.description")
        ),
        command!(
            "play_random",
            rox_i18n::t_static("keymap-play-random"),
            Group::Playback,
            WORKSPACE,
            defaults::PLAY_RANDOM,
            PlayRandom,
            rox_i18n::t_static("keymap-play-random.description")
        ),
        command!(
            "play_similar",
            rox_i18n::t_static("keymap-play-similar"),
            Group::Playback,
            WORKSPACE,
            &[],
            PlaySimilar,
            rox_i18n::t_static("keymap-play-similar.description")
        ),
        command!(
            "toggle_mute",
            rox_i18n::t_static("keymap-toggle-mute"),
            Group::Playback,
            WORKSPACE,
            defaults::MUTE,
            ToggleMute,
            rox_i18n::t_static("keymap-toggle-mute.description")
        ),
        command!(
            "toggle_shuffle",
            rox_i18n::t_static("keymap-toggle-shuffle"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleShuffle,
            rox_i18n::t_static("keymap-toggle-shuffle.description")
        ),
        command!(
            "cycle_shuffle_mode",
            rox_i18n::t_static("keymap-cycle-shuffle-mode"),
            Group::Playback,
            WORKSPACE,
            &[],
            CycleShuffleMode,
            rox_i18n::t_static("keymap-cycle-shuffle-mode.description")
        ),
        command!(
            "cycle_loop",
            rox_i18n::t_static("keymap-cycle-loop"),
            Group::Playback,
            WORKSPACE,
            &[],
            CycleLoop,
            rox_i18n::t_static("keymap-cycle-loop.description")
        ),
        command!(
            "toggle_stop_after",
            rox_i18n::t_static("keymap-toggle-stop-after"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleStopAfter,
            rox_i18n::t_static("keymap-toggle-stop-after.description")
        ),
        // Sleep gets only its cancel: every other sleep row carries a length.
        command!(
            "toggle_continuation",
            rox_i18n::t_static("keymap-toggle-continuation"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleContinuation,
            rox_i18n::t_static("keymap-toggle-continuation.description")
        ),
        command!(
            "sleep_off",
            rox_i18n::t_static("keymap-sleep-off"),
            Group::Playback,
            WORKSPACE,
            &[],
            SleepOff,
            rox_i18n::t_static("keymap-sleep-off.description")
        ),
        command!(
            "clear_queue",
            rox_i18n::t_static("keymap-clear-queue"),
            Group::Playback,
            WORKSPACE,
            &[],
            ClearQueue,
            rox_i18n::t_static("keymap-clear-queue.description")
        ),
        command!(
            "toggle_favourite",
            rox_i18n::t_static("keymap-toggle-favourite"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleFavourite,
            rox_i18n::t_static("keymap-toggle-favourite.description")
        ),
        command!(
            "volume_up",
            rox_i18n::t_static("keymap-volume-up"),
            Group::Playback,
            WORKSPACE,
            &[],
            VolumeUp,
            rox_i18n::t_static("keymap-volume-up.description")
        ),
        command!(
            "volume_down",
            rox_i18n::t_static("keymap-volume-down"),
            Group::Playback,
            WORKSPACE,
            &[],
            VolumeDown,
            rox_i18n::t_static("keymap-volume-down.description")
        ),
        command!(
            "toggle_crossfade",
            rox_i18n::t_static("keymap-toggle-crossfade"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleCrossfade,
            rox_i18n::t_static("keymap-toggle-crossfade.description")
        ),
        command!(
            "toggle_crossfade_albums",
            rox_i18n::t_static("keymap-toggle-crossfade-albums"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleCrossfadeAlbums,
            rox_i18n::t_static("keymap-toggle-crossfade-albums.description")
        ),
        command!(
            "cycle_replaygain_mode",
            rox_i18n::t_static("keymap-cycle-replaygain-mode"),
            Group::Playback,
            WORKSPACE,
            &[],
            CycleReplayGainMode,
            rox_i18n::t_static("keymap-cycle-replaygain-mode.description")
        ),
        // Rebuilds the running session either way; not a quiet switch.
        command!(
            "toggle_exclusive_output",
            rox_i18n::t_static("keymap-toggle-exclusive-output"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleExclusiveOutput,
            rox_i18n::t_static("keymap-toggle-exclusive-output.description")
        ),
        command!(
            "toggle_eq",
            rox_i18n::t_static("keymap-toggle-eq"),
            Group::Playback,
            WORKSPACE,
            &[],
            ToggleEq,
            rox_i18n::t_static("keymap-toggle-eq.description")
        ),
        command!(
            "flatten_eq",
            rox_i18n::t_static("keymap-flatten-eq"),
            Group::Playback,
            WORKSPACE,
            &[],
            FlattenEq,
            rox_i18n::t_static("keymap-flatten-eq.description")
        ),
        // Library operations ship unbound except health and power search: an
        // afternoon-long pass shouldn't be reachable by accident.
        command!(
            "rescan_library",
            rox_i18n::t_static("keymap-rescan-library"),
            Group::Library,
            WORKSPACE,
            &[],
            RescanLibrary,
            rox_i18n::t_static("keymap-rescan-library.description")
        ),
        command!(
            "abort_scan",
            rox_i18n::t_static("keymap-abort-scan"),
            Group::Library,
            WORKSPACE,
            &[],
            AbortScan,
            rox_i18n::t_static("keymap-abort-scan.description")
        ),
        command!(
            "measure_replaygain",
            rox_i18n::t_static("keymap-measure-replaygain"),
            Group::Library,
            WORKSPACE,
            &[],
            MeasureReplayGain,
            rox_i18n::t_static("keymap-measure-replaygain.description")
        ),
        command!(
            "analyze_tempo",
            rox_i18n::t_static("keymap-analyze-tempo"),
            Group::Library,
            WORKSPACE,
            &[],
            AnalyzeTempo,
            rox_i18n::t_static("keymap-analyze-tempo.description")
        ),
        command!(
            "build_acoustic",
            rox_i18n::t_static("keymap-build-acoustic"),
            Group::Library,
            WORKSPACE,
            &[],
            BuildAcoustic,
            rox_i18n::t_static("keymap-build-acoustic.description")
        ),
        command!(
            "fill_sort_names",
            rox_i18n::t_static("keymap-fill-sort-names"),
            Group::Library,
            WORKSPACE,
            &[],
            FillSortNames,
            rox_i18n::t_static("keymap-fill-sort-names.description")
        ),
        command!(
            "romanize_library",
            rox_i18n::t_static("keymap-romanize-library"),
            Group::Library,
            WORKSPACE,
            &[],
            RomanizeLibrary,
            rox_i18n::t_static("keymap-romanize-library.description")
        ),
        command!(
            "find_duplicates",
            rox_i18n::t_static("keymap-find-duplicates"),
            Group::Library,
            WORKSPACE,
            &[],
            FindDuplicates,
            rox_i18n::t_static("keymap-find-duplicates.description")
        ),
        command!(
            "tag_genres",
            rox_i18n::t_static("keymap-tag-genres"),
            Group::Library,
            WORKSPACE,
            &[],
            TagGenres,
            rox_i18n::t_static("keymap-tag-genres.description")
        ),
        command!(
            "open_health",
            rox_i18n::t_static("keymap-open-health"),
            Group::Library,
            WORKSPACE,
            defaults::HEALTH,
            OpenHealth,
            rox_i18n::t_static("keymap-open-health.description")
        ),
        command!(
            "open_power_search",
            rox_i18n::t_static("keymap-open-power-search"),
            Group::Library,
            WORKSPACE,
            defaults::POWER_SEARCH,
            OpenPowerSearch,
            rox_i18n::t_static("keymap-open-power-search.description")
        ),
        command!(
            "type_ahead_next",
            rox_i18n::t_static("keymap-type-ahead-next"),
            Group::Browsing,
            TYPE_AHEAD,
            &["tab"],
            TypeAheadNext,
            rox_i18n::t_static("keymap-type-ahead-next.description"),
            Reach::Panel
        ),
        command!(
            "type_ahead_prev",
            rox_i18n::t_static("keymap-type-ahead-prev"),
            Group::Browsing,
            TYPE_AHEAD,
            &["shift-tab"],
            TypeAheadPrev,
            rox_i18n::t_static("keymap-type-ahead-prev.description"),
            Reach::Panel
        ),
        command!(
            "next_tab",
            rox_i18n::t_static("keymap-next-tab"),
            Group::Browsing,
            WORKSPACE,
            &["ctrl-tab"],
            NextTab,
            rox_i18n::t_static("keymap-next-tab.description"),
            Reach::Panel
        ),
        command!(
            "prev_tab",
            rox_i18n::t_static("keymap-prev-tab"),
            Group::Browsing,
            WORKSPACE,
            &["ctrl-shift-tab"],
            PrevTab,
            rox_i18n::t_static("keymap-prev-tab.description"),
            Reach::Panel
        ),
        command!(
            "close_panel",
            rox_i18n::t_static("keymap-close-panel"),
            Group::Browsing,
            WORKSPACE,
            &[],
            ClosePanelAction,
            rox_i18n::t_static("keymap-close-panel.description"),
            Reach::Panel
        ),
        command!(
            "new_window",
            rox_i18n::t_static("keymap-new-window"),
            Group::Windows,
            WORKSPACE,
            defaults::NEW_WINDOW,
            NewWindow,
            rox_i18n::t_static("keymap-new-window.description")
        ),
        command!(
            "new_empty_window",
            rox_i18n::t_static("keymap-new-empty-window"),
            Group::Windows,
            WORKSPACE,
            &[],
            NewEmptyWindow,
            rox_i18n::t_static("keymap-new-empty-window.description")
        ),
        command!(
            "open_tasks",
            rox_i18n::t_static("keymap-open-tasks"),
            Group::Windows,
            WORKSPACE,
            defaults::TASKS,
            OpenTasks,
            rox_i18n::t_static("keymap-open-tasks.description")
        ),
        command!(
            "open_equalizer",
            rox_i18n::t_static("keymap-open-equalizer"),
            Group::Windows,
            WORKSPACE,
            defaults::EQUALIZER,
            OpenEqualizer,
            rox_i18n::t_static("keymap-open-equalizer.description")
        ),
        command!(
            "open_preset_picker",
            rox_i18n::t_static("keymap-open-preset-picker"),
            Group::Windows,
            WORKSPACE,
            defaults::PRESET_PICKER,
            OpenPresetPicker,
            rox_i18n::t_static("keymap-open-preset-picker.description")
        ),
        command!(
            "open_console",
            rox_i18n::t_static("keymap-open-console"),
            Group::Windows,
            WORKSPACE,
            &["f12"],
            OpenConsole,
            rox_i18n::t_static("keymap-open-console.description")
        ),
        command!(
            "open_welcome",
            rox_i18n::t_static("keymap-open-welcome"),
            Group::Windows,
            WORKSPACE,
            &["f1"],
            OpenWelcome,
            rox_i18n::t_static("keymap-open-welcome.description")
        ),
        command!(
            "open_about",
            rox_i18n::t_static("keymap-open-about"),
            Group::Windows,
            WORKSPACE,
            &["shift-f1"],
            OpenAbout,
            rox_i18n::t_static("keymap-open-about.description")
        ),
        command!(
            "report_issue",
            rox_i18n::t_static("keymap-report-issue"),
            Group::Windows,
            WORKSPACE,
            &[],
            ReportIssue,
            rox_i18n::t_static("keymap-report-issue.description")
        ),
        command!(
            "open_discussions",
            rox_i18n::t_static("keymap-open-discussions"),
            Group::Windows,
            WORKSPACE,
            &[],
            OpenDiscussions,
            rox_i18n::t_static("keymap-open-discussions.description")
        ),
        command!(
            "open_chat",
            rox_i18n::t_static("keymap-open-chat"),
            Group::Windows,
            WORKSPACE,
            &[],
            OpenChat,
            rox_i18n::t_static("keymap-open-chat.description")
        ),
        command!(
            "open_settings",
            rox_i18n::t_static("keymap-open-settings"),
            Group::Windows,
            WORKSPACE,
            defaults::SETTINGS,
            OpenSettings,
            rox_i18n::t_static("keymap-open-settings.description")
        ),
        command!(
            "open_panel_settings",
            rox_i18n::t_static("keymap-open-panel-settings"),
            Group::Windows,
            WORKSPACE,
            defaults::PANEL_SETTINGS,
            OpenPanelSettings,
            rox_i18n::t_static("keymap-open-panel-settings.description"),
            Reach::Panel
        ),
        command!(
            "open_stats",
            rox_i18n::t_static("keymap-open-stats"),
            Group::Windows,
            WORKSPACE,
            defaults::STATS,
            OpenStats,
            rox_i18n::t_static("keymap-open-stats.description")
        ),
        command!(
            "open_quick_play",
            rox_i18n::t_static("keymap-open-quick-play"),
            Group::Library,
            WORKSPACE,
            defaults::QUICK_PLAY,
            OpenQuickPlay,
            rox_i18n::t_static("keymap-open-quick-play.description")
        ),
        command!(
            "open_signals",
            rox_i18n::t_static("keymap-open-signals"),
            Group::Windows,
            WORKSPACE,
            &[],
            OpenSignals,
            rox_i18n::t_static("keymap-open-signals.description")
        ),
        command!(
            "import_workspace",
            rox_i18n::t_static("keymap-import-workspace"),
            Group::Windows,
            WORKSPACE,
            &[],
            ImportWorkspace,
            rox_i18n::t_static("keymap-import-workspace.description")
        ),
        command!(
            "save_layout",
            rox_i18n::t_static("keymap-save-layout"),
            Group::Windows,
            WORKSPACE,
            &[],
            SaveLayout,
            rox_i18n::t_static("keymap-save-layout.description")
        ),
        command!(
            "save_workspace",
            rox_i18n::t_static("keymap-save-workspace"),
            Group::Windows,
            WORKSPACE,
            &[],
            SaveWorkspace,
            rox_i18n::t_static("keymap-save-workspace.description")
        ),
        command!(
            "toggle_quit_to_tray",
            rox_i18n::t_static("keymap-toggle-quit-to-tray"),
            Group::Windows,
            WORKSPACE,
            &[],
            ToggleQuitToTray,
            rox_i18n::t_static("keymap-toggle-quit-to-tray.description")
        ),
        command!(
            "close_window",
            rox_i18n::t_static("keymap-close-window"),
            Group::Windows,
            None,
            defaults::CLOSE_WINDOW,
            CloseWindow,
            rox_i18n::t_static("keymap-close-window.description")
        ),
        command!(
            "quit",
            rox_i18n::t_static("keymap-quit"),
            Group::Windows,
            None,
            defaults::QUIT,
            Quit,
            rox_i18n::t_static("keymap-quit.description")
        ),
        command!(
            "focus_search",
            rox_i18n::t_static("keymap-focus-search"),
            Group::View,
            WORKSPACE,
            defaults::FOCUS_SEARCH,
            FocusSearch,
            rox_i18n::t_static("keymap-focus-search.description")
        ),
        command!(
            "toggle_zoom",
            rox_i18n::t_static("keymap-toggle-zoom"),
            Group::View,
            WORKSPACE,
            &["shift-escape"],
            ToggleZoom,
            rox_i18n::t_static("keymap-toggle-zoom.description"),
            Reach::Panel
        ),
        command!(
            "increase_font_size",
            rox_i18n::t_static("keymap-increase-font-size"),
            Group::View,
            None,
            defaults::ZOOM_IN,
            IncreaseFontSize,
            rox_i18n::t_static("keymap-increase-font-size.description")
        ),
        command!(
            "decrease_font_size",
            rox_i18n::t_static("keymap-decrease-font-size"),
            Group::View,
            None,
            defaults::ZOOM_OUT,
            DecreaseFontSize,
            rox_i18n::t_static("keymap-decrease-font-size.description")
        ),
        command!(
            "reset_font_size",
            rox_i18n::t_static("keymap-reset-font-size"),
            Group::View,
            None,
            defaults::ZOOM_RESET,
            ResetFontSize,
            rox_i18n::t_static("keymap-reset-font-size.description")
        ),
        command!(
            "toggle_post_shader",
            rox_i18n::t_static("keymap-toggle-post-shader"),
            Group::View,
            None,
            defaults::POST_SHADER,
            TogglePostShader,
            rox_i18n::t_static("keymap-toggle-post-shader.description")
        ),
        command!(
            "toggle_theme",
            rox_i18n::t_static("keymap-toggle-theme"),
            Group::View,
            None,
            defaults::THEME,
            ToggleTheme,
            rox_i18n::t_static("keymap-toggle-theme.description")
        ),
        command!(
            "toggle_design_mode",
            rox_i18n::t_static("keymap-toggle-design-mode"),
            Group::View,
            WORKSPACE,
            defaults::DESIGN_MODE,
            ToggleDesignMode,
            rox_i18n::t_static("keymap-toggle-design-mode.description")
        ),
        command!(
            "toggle_resize_lock",
            rox_i18n::t_static("keymap-toggle-resize-lock"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleResizeLock,
            rox_i18n::t_static("keymap-toggle-resize-lock.description")
        ),
        command!(
            "toggle_menubar",
            rox_i18n::t_static("keymap-toggle-menubar"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleMenubar,
            rox_i18n::t_static("keymap-toggle-menubar.description")
        ),
        command!(
            "toggle_decorations",
            rox_i18n::t_static("keymap-toggle-decorations"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleDecorations,
            rox_i18n::t_static("keymap-toggle-decorations.description")
        ),
        command!(
            "toggle_art_theming",
            rox_i18n::t_static("keymap-toggle-art-theming"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleArtTheming,
            rox_i18n::t_static("keymap-toggle-art-theming.description")
        ),
        command!(
            "toggle_seams",
            rox_i18n::t_static("keymap-toggle-seams"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleSeams,
            rox_i18n::t_static("keymap-toggle-seams.description")
        ),
        command!(
            "toggle_readings",
            rox_i18n::t_static("keymap-toggle-readings"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleReadings,
            rox_i18n::t_static("keymap-toggle-readings.description")
        ),
        // A window with neither the mini nor the primary preset named stays put.
        command!(
            "toggle_mini",
            rox_i18n::t_static("keymap-toggle-mini"),
            Group::View,
            WORKSPACE,
            &[],
            ToggleMini,
            rox_i18n::t_static("keymap-toggle-mini.description")
        ),
        command!(
            "stamp_line",
            rox_i18n::t_static("keymap-stamp-line"),
            Group::Editing,
            LYRICS,
            &["shift-enter"],
            StampLine,
            rox_i18n::t_static("keymap-stamp-line.description"),
            Reach::Panel
        ),
    ]
});

/// Entries from another build that this one doesn't know stay in the file,
/// unbound.
pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// Only rejects the empty chord: gpui's own parse takes any word as a key,
/// so "ctrl-nonsense" binds and never fires.
pub fn parses(chord: &str) -> bool {
    let mut keystrokes = chord.split_whitespace().peekable();
    keystrokes.peek().is_some() && keystrokes.all(|key| Keystroke::parse(key).is_ok())
}

pub fn chords(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    match overrides.get(command.id) {
        Some(chords) => chords.clone(),
        None => command.defaults.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn is_default(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> bool {
    match overrides.get(command.id) {
        Some(chords) => chords.as_slice() == command.defaults,
        None => true,
    }
}

/// [`PLAYBACK`] and [`SEEK`] are subsets of [`WORKSPACE`], so they overlap it
/// and each other. Unscoped overlaps everything.
fn overlaps(a: Option<&'static str>, b: Option<&'static str>) -> bool {
    let widen = |scope| if narrowed(scope) { WORKSPACE } else { scope };
    a.is_none() || b.is_none() || widen(a) == widen(b)
}

/// Both sides resolve through [`Command::scope`], so widening is accounted for.
pub fn clash(
    command: &Command,
    chord: &str,
    overrides: &BTreeMap<String, Vec<String>>,
) -> Option<&'static str> {
    let scope = command.scope(chord);
    COMMANDS
        .iter()
        .filter(|other| other.id != command.id)
        .find(|other| {
            chords(other, overrides)
                .iter()
                .any(|held| held == chord && overlaps(other.scope(held), scope))
        })
        .map(|other| other.label)
}

struct Layers {
    /// The widget library's bindings as [`init`] found them. Losing these costs typing.
    library: Vec<KeyBinding>,
    strays: Vec<KeyBinding>,
    /// Anything past this in the live keymap was bound behind this module's back.
    laid: usize,
}

impl Global for Layers {}

/// The windows' own chords the Keymap page doesn't offer. Above the widget
/// layer, which lets the shader editor's apply take ctrl-enter from its input.
fn fixed() -> Vec<KeyBinding> {
    [
        crate::tags::editor::bindings(),
        crate::tags::rename::bindings(),
        crate::tags::repair::bindings(),
        crate::smart_playlist::bindings(),
        crate::playlist_create::bindings(),
        crate::bookmark_dialog::bindings(),
        crate::bake_dialog::bindings(),
        crate::convert_dialog::bindings(),
        crate::lyrics::edit::bindings(),
        crate::lyrics::matcher::bindings(),
        crate::shader_editor::bindings(),
        crate::cover::editor::bindings(),
        crate::settings::shader_confirm::bindings(),
        rox_panel_api::panel_settings::bindings(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Call after the widget library's init, so its bindings land in the bottom layer.
pub fn init(cx: &mut App) {
    seed(cx);
    apply(cx);
}

/// Apart from the settings read so a test can seed a keymap.
fn seed(cx: &mut App) {
    let library: Vec<KeyBinding> = cx.key_bindings().borrow().bindings().cloned().collect();

    // Still live until the first rebuild clears them, so they count as laid.
    let laid = library.len();

    cx.set_global(Layers {
        library,
        strays: Vec::new(),
        laid,
    });
}

/// Each command's first chord for display, refilled by [`apply`]. Menus read
/// it every frame, where a settings load has no place.
static SHORTCUTS: RwLock<BTreeMap<&'static str, String>> = RwLock::new(BTreeMap::new());

pub fn shortcut(id: &str) -> Option<String> {
    SHORTCUTS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(id)
        .cloned()
}

pub fn apply(cx: &mut App) {
    let overrides = Settings::load().keymap;
    rebuild(&overrides, cx);
}

fn rebuild(overrides: &BTreeMap<String, Vec<String>>, cx: &mut App) {
    adopt_strays(cx);

    let layers = cx.global::<Layers>();
    let mut bindings = layers.library.clone();
    bindings.extend(fixed());

    let mut shortcuts = BTreeMap::new();
    for command in COMMANDS.iter() {
        let chords = chords(command, overrides);
        if let Some(chord) = chords.first() {
            shortcuts.insert(command.id, display(chord));
        }
        bindings.extend(chords.iter().filter_map(|chord| command.binding(chord)));
    }
    *SHORTCUTS.write().unwrap_or_else(PoisonError::into_inner) = shortcuts;

    // Strays go back on top so a shared chord's winner doesn't change.
    bindings.extend(layers.strays.iter().cloned());

    cx.global_mut::<Layers>().laid = bindings.len();
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// gpui only appends, so anything past the last rebuild's count arrived after it.
fn adopt_strays(cx: &mut App) {
    let laid = cx.global::<Layers>().laid;
    let strays: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .skip(laid)
        .cloned()
        .collect();

    if strays.is_empty() {
        return;
    }

    for stray in &strays {
        log::warn!(
            "keymap: {stray:?} was bound outside keymap.rs after keymap::init; \
             carried through the rebind, but it belongs in keymap::fixed"
        );
    }
    cx.global_mut::<Layers>().strays.extend(strays);
}

pub fn add(id: &str, chord: String, cx: &mut App) {
    edit(id, cx, move |chords| {
        if !chords.contains(&chord) {
            chords.push(chord);
        }
    });
}

pub fn remove(id: &str, chord: &str, cx: &mut App) {
    let chord = chord.to_string();
    edit(id, cx, move |chords| chords.retain(|held| *held != chord));
}

pub fn reset(id: &str, cx: &mut App) {
    let id = id.to_string();
    Settings::update(move |settings| {
        settings.keymap.remove(&id);
    });
    apply(cx);
}

/// Clears the whole map, including commands this build doesn't know.
pub fn reset_all(cx: &mut App) {
    Settings::update(|settings| settings.keymap.clear());
    apply(cx);
}

/// The undo for a reset.
pub fn restore(map: BTreeMap<String, Vec<String>>, cx: &mut App) {
    Settings::update(move |settings| settings.keymap = map);
    apply(cx);
}

/// Seeds from the defaults, so a first edit keeps the other chords.
fn edit(id: &str, cx: &mut App, change: impl FnOnce(&mut Vec<String>) + Send + 'static) {
    let id = id.to_string();
    let defaults: Vec<String> = command(&id)
        .map(|command| command.defaults.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    Settings::update(move |settings| {
        let chords = settings.keymap.entry(id).or_insert(defaults);
        change(chords);
    });
    apply(cx);
}

/// "ctrl-shift-s" reads "Ctrl+Shift+S"; sequence parts stay space-separated.
pub fn display(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(|key| match Keystroke::parse(key) {
            Ok(keystroke) => display_keystroke(&keystroke),
            Err(_) => key.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_keystroke(keystroke: &Keystroke) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let modifiers = keystroke.modifiers;
    if modifiers.control {
        parts.push(rox_i18n::t_static("keymap-mod-ctrl"));
    }
    if modifiers.alt {
        parts.push(if cfg!(target_os = "macos") {
            rox_i18n::t_static("keymap-mod-option")
        } else {
            rox_i18n::t_static("keymap-mod-alt")
        });
    }
    if modifiers.shift {
        parts.push(rox_i18n::t_static("keymap-mod-shift"));
    }
    if modifiers.platform {
        parts.push(match () {
            _ if cfg!(target_os = "macos") => rox_i18n::t_static("keymap-mod-cmd"),
            _ if cfg!(target_os = "windows") => rox_i18n::t_static("keymap-mod-win"),
            _ => rox_i18n::t_static("keymap-mod-super"),
        });
    }
    if modifiers.function {
        parts.push(rox_i18n::t_static("keymap-mod-fn"));
    }
    let key = key_label(&keystroke.key);
    parts.push(&key);
    parts.join("+")
}

fn key_label(key: &str) -> String {
    match key {
        "escape" => rox_i18n::t!("keymap-key-esc").to_string(),
        "enter" => "Enter".to_string(),
        "backspace" => rox_i18n::t!("keymap-key-backspace").to_string(),
        "delete" => rox_i18n::t!("keymap-key-delete").to_string(),
        "space" => rox_i18n::t!("keymap-key-space").to_string(),
        "tab" => rox_i18n::t!("keymap-key-tab").to_string(),
        "up" => rox_i18n::t!("keymap-key-up").to_string(),
        "down" => rox_i18n::t!("keymap-key-down").to_string(),
        "left" => rox_i18n::t!("keymap-key-left").to_string(),
        "right" => rox_i18n::t!("keymap-key-right").to_string(),
        "pageup" => rox_i18n::t!("keymap-key-page-up").to_string(),
        "pagedown" => rox_i18n::t!("keymap-key-page-down").to_string(),
        "home" => rox_i18n::t!("keymap-key-home").to_string(),
        "end" => rox_i18n::t!("keymap-key-end").to_string(),
        "insert" => rox_i18n::t!("keymap-key-insert").to_string(),
        key if key.len() == 1 => key.to_uppercase(),
        key => {
            let mut chars = key.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{TestAppContext, actions};

    actions!(keymap_test, [LibraryKey, StrayKey]);

    fn position(cx: &App, action: &dyn Action) -> Option<usize> {
        cx.key_bindings()
            .borrow()
            .bindings()
            .position(|binding| binding.action().partial_eq(action))
    }

    /// Two rebuilds, so a stray adopted on the first survives the second undoubled.
    #[gpui::test]
    fn a_rebind_keeps_every_layer(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new("ctrl-y", LibraryKey, None)]);
            seed(cx);
            rebuild(&BTreeMap::new(), cx);

            cx.bind_keys([KeyBinding::new("ctrl-u", StrayKey, None)]);
            rebuild(&BTreeMap::new(), cx);
            rebuild(&BTreeMap::new(), cx);

            let library = position(cx, &LibraryKey).expect("the library layer survived");
            let window =
                position(cx, &crate::bake_dialog::Embed).expect("a window's fixed chord survived");
            let command = position(cx, &TogglePlayback).expect("the commands survived");
            let stray = position(cx, &StrayKey).expect("the stray was carried forward");

            assert!(
                library < window,
                "the widget library sits under rox's windows"
            );
            assert!(window < command, "the fixed chords sit under the commands");
            assert!(command < stray, "a stray stays on top, where it was bound");

            let strays = cx
                .key_bindings()
                .borrow()
                .bindings()
                .filter(|binding| binding.action().partial_eq(&StrayKey))
                .count();
            assert_eq!(strays, 1, "a second rebuild adopted the stray again");
        });
    }

    /// Commands handled on a panel or another window. Everything else answers
    /// from the app or the workspace root, which every button sits under.
    const PANEL_SCOPED: &[&str] = &[
        "type_ahead_next",
        "type_ahead_prev",
        "next_tab",
        "prev_tab",
        "close_panel",
        "open_panel_settings",
        "toggle_zoom",
        "stamp_line",
    ];

    /// A Global command handled on a panel gives a button that silently does nothing.
    #[test]
    fn every_command_declares_a_reach() {
        for command in COMMANDS.iter() {
            let expected = if PANEL_SCOPED.contains(&command.id) {
                Reach::Panel
            } else {
                Reach::Global
            };

            assert!(
                command.reach == expected,
                "{} declares the wrong reach",
                command.id
            );
        }
    }

    #[test]
    fn dispatch_refuses_an_unknown_id() {
        assert!(
            !COMMANDS
                .iter()
                .any(|command| command.id == "no_such_command"),
            "the fixture id has become a real command"
        );
    }

    #[test]
    fn global_commands_excludes_the_panel_tier() {
        assert_eq!(
            global_commands().count(),
            COMMANDS.len() - PANEL_SCOPED.len()
        );
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<&str> = COMMANDS.iter().map(|command| command.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "two commands share a settings id");
    }

    #[test]
    fn defaults_parse() {
        for command in COMMANDS.iter() {
            for chord in command.defaults {
                assert!(parses(chord), "{} ships an unbindable {chord}", command.id);
            }
        }
    }

    #[test]
    fn defaults_do_not_clash() {
        let overrides = BTreeMap::new();
        for command in COMMANDS.iter() {
            for chord in command.defaults {
                assert_eq!(
                    clash(command, chord, &overrides),
                    None,
                    "{} shares {chord} with another command",
                    command.label
                );
            }
        }
    }

    #[test]
    fn display_reads_as_keycaps() {
        assert_eq!(
            display("ctrl-shift-s"),
            format!(
                "{}+{}+S",
                rox_i18n::t_static("keymap-mod-ctrl"),
                rox_i18n::t_static("keymap-mod-shift")
            )
        );
        assert_eq!(
            display("space"),
            rox_i18n::t!("keymap-key-space").to_string()
        );
        assert_eq!(
            display("shift-escape"),
            format!(
                "{}+{}",
                rox_i18n::t_static("keymap-mod-shift"),
                rox_i18n::t!("keymap-key-esc")
            )
        );
    }

    #[test]
    fn empty_chords_do_not_parse() {
        assert!(!parses(""));
        assert!(!parses("   "));
    }

    #[test]
    fn loose_chords_still_bind() {
        assert!(parses("ctrl"));
        assert!(parses("ctrl-nonsense"));
        assert!(parses("ctrl-k ctrl-s"), "a two-keystroke sequence");
    }

    #[test]
    fn shift_alone_is_typing() {
        assert!(!modified("space"));
        assert!(!modified("left"));
        assert!(!modified("shift-left"));
        assert!(modified("ctrl-f"));
        assert!(modified("alt-left"));
        assert!(modified("cmd-space"));
    }

    #[test]
    fn sequences_read_their_opening_chord() {
        assert!(modified("ctrl-k left"));
        assert!(!modified("g ctrl-f"));
    }

    #[test]
    fn modified_playback_chords_reach_the_search_box() {
        let seek = COMMANDS
            .iter()
            .find(|c| c.id == "seek_forward")
            .expect("seek_forward is bound");
        assert_eq!(seek.context, SEEK, "the fixture moved scope");
        assert_eq!(seek.scope("right"), SEEK);
        assert_eq!(seek.scope("ctrl-f"), WORKSPACE);
    }

    #[test]
    fn widened_chords_clash_with_the_workspace() {
        let seek = COMMANDS
            .iter()
            .find(|c| c.id == "seek_forward")
            .expect("seek_forward is bound");
        let overrides = BTreeMap::new();
        assert_eq!(
            clash(seek, "ctrl-l", &overrides),
            Some(rox_i18n::t_static("keymap-focus-search")),
            "a modified playback chord binds where Focus Search lives"
        );
        assert_eq!(
            clash(seek, "right", &overrides),
            None,
            "a bare one still bows out of the search box"
        );
    }

    #[test]
    fn other_scopes_do_not_widen() {
        for command in COMMANDS.iter().filter(|c| !narrowed(c.context)) {
            assert_eq!(
                command.scope("ctrl-f"),
                command.context,
                "{} widened out of its own scope",
                command.id
            );
        }
    }
}
