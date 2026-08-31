//! Every chord rox binds, in one list, so the Keymap settings page has
//! something to draw and the settings file has something to override.
//!
//! Bindings used to be a `bind_keys` call at startup with the platform
//! forks inline. They still are, except the call is built from
//! [`COMMANDS`] rather than written out, and each command's chords come
//! from the settings file when it has an opinion and from the command's
//! own defaults when it doesn't. That split is the whole design: the file
//! only ever holds what someone changed, so a default that moves in a
//! later build applies to everyone who left it alone.
//!
//! Rebinding at runtime means rebuilding the keymap, because gpui only
//! offers add and clear: there's no remove. Clearing takes the widget
//! library's bindings with it (every text input's editing keys are in
//! there), so [`init`] snapshots what was already registered before rox
//! adds its own, and every rebuild lays that snapshot back down first.
//! Anything binding keys after [`init`] runs would be lost on the first
//! rebind; today nothing does, and this is the note explaining why the
//! init order in `main` matters.

use std::collections::BTreeMap;
use std::sync::{LazyLock, PoisonError, RwLock};

use gpui::{App, Global, KeyBinding, Keystroke};

use rox_core::settings::Settings;

use rox_dock::{NextTab, OpenPanelSettings, PrevTab, ToggleZoom};
use rox_panel_api::actions::{
    SeekBackward, SeekForward, TogglePlayback, TypeAheadNext, TypeAheadPrev,
};
use rox_panels::lyrics::StampLine;

use crate::workspace::{
    CloseWindow, DecreaseFontSize, FocusSearch, IncreaseFontSize, NewWindow, NextTrack, OpenAbout,
    OpenConsole, OpenEqualizer, OpenQuickPlay, OpenSettings, OpenStats, OpenTasks, OpenWelcome,
    PlayRandom, PreviousTrack, Quit, ResetFontSize, StopPlayback, TogglePostShader,
};

/// Bindings match key contexts along the focus path, so this scope holds
/// anywhere inside a workspace window except while the library search box
/// is focused, a browsing panel's type-ahead phrase is mid-flight, the
/// menubar is taking keys, or a button or slider has been tabbed to:
/// there space and arrows keep typing into the query or the phrase, walk
/// the menus, or press the control, instead. Bindings win over key
/// listeners, so the exclusion hands the keys back.
///
/// The exclusion is for bare chords only. A command rebound onto a
/// modified chord widens to [`WORKSPACE`] at build time, since ctrl-f
/// isn't anything the search box needs and losing the binding while you
/// type is the whole complaint. See [`Command::binding`].
const PLAYBACK: Option<&str> =
    Some("Workspace && !SearchInput && !TypeAhead && !MenuNav && !FocusedControl");

/// [`PLAYBACK`] minus the panels whose own left and right mean something:
/// a tile wall moving its cursor across a row, a folder tree folding a
/// branch. Only the seek pair sits here, since it's the only bare chord
/// that collides; space stays on [`PLAYBACK`] so play/pause still works
/// with a wall focused.
const SEEK: Option<&str> =
    Some("Workspace && !SearchInput && !TypeAhead && !MenuNav && !PanelNav && !FocusedControl");

/// The plain workspace scope: anywhere in a workspace window, the search
/// box included, since everything bound here has a modifier.
const WORKSPACE: Option<&str> = Some("Workspace");

/// The lyrics editor's own scope, deeper along the focus path than the
/// window root.
const LYRICS: Option<&str> = Some("LyricsEdit");

/// Where the type-ahead cycle binds: a panel carries this only while it
/// holds a phrase, so tab steps matches then and goes back to walking
/// panels the rest of the time.
const TYPE_AHEAD: Option<&str> = Some(rox_panel_kit::TYPE_AHEAD_CYCLE_CONTEXT);

/// Which part of the app a command belongs to. The Keymap page draws one
/// section per group, in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Playback,
    Windows,
    Browsing,
    View,
    Editing,
}

impl Group {
    /// The groups the page steps through, in the order it draws them.
    pub const ALL: &'static [Group] = &[
        Group::Playback,
        Group::Windows,
        Group::Browsing,
        Group::View,
        Group::Editing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Group::Playback => rox_i18n::t_static("keymap-group-playback"),
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
            Group::Windows => icons::APP_WINDOW,
            Group::Browsing => icons::SEARCH,
            Group::View => icons::EYE,
            Group::Editing => icons::PENCIL,
        }
    }
}

/// One rebindable thing rox can do.
pub struct Command {
    /// The settings file's key for this command. Stable forever: renaming
    /// one silently resets everyone who had rebound it, so a label change
    /// must not touch this.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub group: Group,
    /// Where the chord is live. `None` is everywhere, including the
    /// settings and about windows and popped-out panels.
    pub context: Option<&'static str>,
    /// The chords this command ships with, in gpui's own syntax
    /// ("ctrl-shift-s"). More than one is an alias, not a sequence.
    pub defaults: &'static [&'static str],
    /// Builds the binding for one chord. Each command names a distinct
    /// action type, so the type has to be baked in here rather than
    /// stored as data.
    build: fn(&str, Option<&'static str>) -> KeyBinding,
}

impl Command {
    /// The binding for one chord, or `None` when the chord doesn't parse.
    /// A file edited by hand is the way that happens, and dropping the
    /// one bad line beats refusing to bind anything.
    fn binding(&self, chord: &str) -> Option<KeyBinding> {
        parses(chord).then(|| (self.build)(chord, self.scope(chord)))
    }

    /// The context this chord binds under. Everything binds under the
    /// command's own scope except a modified chord on one of the narrowed
    /// workspace scopes, which widens to the whole workspace: the
    /// exclusions are there so space and the arrows keep reaching the query,
    /// the phrase and the cursor, and a chord holding ctrl, alt or cmd was
    /// never going to reach any of them anyway.
    fn scope(&self, chord: &str) -> Option<&'static str> {
        if narrowed(self.context) && modified(chord) {
            WORKSPACE
        } else {
            self.context
        }
    }
}

/// Whether a scope is [`WORKSPACE`] with exclusions carved out of it, the
/// scopes a modified chord widens back out of.
fn narrowed(scope: Option<&'static str>) -> bool {
    scope == PLAYBACK || scope == SEEK
}

/// Whether a chord opens on a modified keystroke. Shift doesn't count:
/// shift-letter is typing, and a text box needs it. The first keystroke
/// decides, since that's the one a focused input would otherwise eat.
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
    ($id:literal, $label:expr, $group:expr, $ctx:expr, $keys:expr, $action:expr, $desc:expr) => {
        Command {
            id: $id,
            label: $label,
            description: $desc,
            group: $group,
            context: $ctx,
            defaults: $keys,
            build: |keys, ctx| KeyBinding::new(keys, $action, ctx),
        }
    };
}

// The platform forks, pulled out of the list below so each command reads
// as one line. macOS puts app-level chords on Cmd; everywhere else
// they're on Ctrl.
#[cfg(target_os = "macos")]
mod defaults {
    pub const SETTINGS: &[&str] = &["cmd-,", "ctrl-i"];
    pub const PANEL_SETTINGS: &[&str] = &["cmd-shift-,"];
    pub const STATS: &[&str] = &["cmd-shift-s"];
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
    pub const NEXT_TRACK: &[&str] = &["cmd-right"];
    pub const PREVIOUS_TRACK: &[&str] = &["cmd-left"];
    pub const STOP: &[&str] = &["cmd-."];
    pub const PLAY_RANDOM: &[&str] = &["cmd-r"];
}

#[cfg(not(target_os = "macos"))]
mod defaults {
    pub const SETTINGS: &[&str] = &["ctrl-,", "ctrl-i"];
    pub const PANEL_SETTINGS: &[&str] = &["ctrl-<"];
    pub const STATS: &[&str] = &["ctrl-shift-s"];
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
    pub const NEXT_TRACK: &[&str] = &["ctrl-right"];
    pub const PREVIOUS_TRACK: &[&str] = &["ctrl-left"];
    pub const STOP: &[&str] = &["ctrl-."];
    pub const PLAY_RANDOM: &[&str] = &["ctrl-r"];
}

/// Everything rox binds. The page draws this in order within each group,
/// so related rows sit together.
///
/// A `Vec` behind [`LazyLock`] rather than a `const` slice, because the
/// label and description strings resolve through the locale bundles at
/// first use, and that lookup isn't `const fn`. Every `.iter()` call site
/// goes through [`LazyLock`]'s deref to the built `Vec`; a bare
/// `for command in COMMANDS` needs `.iter()` added, since deref coercion
/// doesn't apply to `IntoIterator`.
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
        command!(
            "stop_playback",
            rox_i18n::t_static("keymap-stop-playback"),
            Group::Playback,
            WORKSPACE,
            defaults::STOP,
            StopPlayback,
            rox_i18n::t_static("keymap-stop-playback.description")
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
            "type_ahead_next",
            rox_i18n::t_static("keymap-type-ahead-next"),
            Group::Browsing,
            TYPE_AHEAD,
            &["tab"],
            TypeAheadNext,
            rox_i18n::t_static("keymap-type-ahead-next.description")
        ),
        command!(
            "type_ahead_prev",
            rox_i18n::t_static("keymap-type-ahead-prev"),
            Group::Browsing,
            TYPE_AHEAD,
            &["shift-tab"],
            TypeAheadPrev,
            rox_i18n::t_static("keymap-type-ahead-prev.description")
        ),
        command!(
            "next_tab",
            rox_i18n::t_static("keymap-next-tab"),
            Group::Browsing,
            WORKSPACE,
            &["ctrl-tab"],
            NextTab,
            rox_i18n::t_static("keymap-next-tab.description")
        ),
        command!(
            "prev_tab",
            rox_i18n::t_static("keymap-prev-tab"),
            Group::Browsing,
            WORKSPACE,
            &["ctrl-shift-tab"],
            PrevTab,
            rox_i18n::t_static("keymap-prev-tab.description")
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
            rox_i18n::t_static("keymap-open-panel-settings.description")
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
            Group::Windows,
            WORKSPACE,
            defaults::QUICK_PLAY,
            OpenQuickPlay,
            rox_i18n::t_static("keymap-open-quick-play.description")
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
            rox_i18n::t_static("keymap-toggle-zoom.description")
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
            "stamp_line",
            rox_i18n::t_static("keymap-stamp-line"),
            Group::Editing,
            LYRICS,
            &["shift-enter"],
            StampLine,
            rox_i18n::t_static("keymap-stamp-line.description")
        ),
    ]
});

/// The command with this id, if the registry still has one. A settings
/// file written by an older or newer build can name commands this one
/// doesn't know; those entries stay in the file untouched and just aren't
/// bound.
pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// Whether a chord is one gpui can bind. Whitespace separates the
/// keystrokes of a sequence, so every part has to parse on its own.
///
/// This is a shape check, and a loose one, because gpui's own parse is
/// loose: any word that isn't a modifier is taken as a key name, so
/// "ctrl-nonsense" binds cleanly and never fires, and a bare "ctrl" is a
/// real binding on a modifier tap. That leaves only the empty chord to
/// reject, which is what an emptied field in a hand-edited file leaves
/// behind.
pub fn parses(chord: &str) -> bool {
    let mut keystrokes = chord.split_whitespace().peekable();
    keystrokes.peek().is_some() && keystrokes.all(|key| Keystroke::parse(key).is_ok())
}

/// The chords a command is running: the file's when it has an opinion,
/// the command's own defaults when it doesn't.
pub fn chords(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    match overrides.get(command.id) {
        Some(chords) => chords.clone(),
        None => command.defaults.iter().map(|s| s.to_string()).collect(),
    }
}

/// Whether a command still has exactly what it ships with, which decides
/// if the page offers a reset.
pub fn is_default(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> bool {
    match overrides.get(command.id) {
        Some(chords) => chords.as_slice() == command.defaults,
        None => true,
    }
}

/// Whether two scopes can both be live at one moment. Unscoped is live
/// everywhere and so overlaps anything. [`PLAYBACK`] and [`SEEK`] are
/// [`WORKSPACE`] with exclusions carved out, subsets rather than
/// neighbours, so they overlap it and each other wherever a workspace
/// window has focus. Everything else here is a distinct window or editor
/// and only overlaps itself.
fn overlaps(a: Option<&'static str>, b: Option<&'static str>) -> bool {
    let widen = |scope| if narrowed(scope) { WORKSPACE } else { scope };
    a.is_none() || b.is_none() || widen(a) == widen(b)
}

/// Another command holding the same chord somewhere this one is also
/// live, if there is one. Both sides resolve through
/// [`Command::scope`] first, so a playback command rebound onto a
/// modified chord is checked where it actually binds rather than where it
/// was declared.
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

/// The bindings that were already registered when rox's keymap took over:
/// the widget library's text editing keys, the dock's, the tag editor's
/// tab. A rebind clears the keymap wholesale, so these have to be laid
/// back down with it or the app loses the ability to type.
struct Foreign(Vec<KeyBinding>);

impl Global for Foreign {}

/// Snapshot what's already bound, then bind rox's own set. Call once at
/// startup, after everything else that binds keys.
pub fn init(cx: &mut App) {
    let foreign = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(Foreign(foreign));
    apply(cx);
}

/// Each command's leading chord, written out the way a person reads it,
/// keyed by command id. The menus trail their rows with this, and they
/// rebuild every frame the dropdown is up, where a settings-file load has
/// no place. [`apply`] refills it, so a rebind moves the menu label with
/// the binding instead of leaving the two disagreeing.
static SHORTCUTS: RwLock<BTreeMap<&'static str, String>> = RwLock::new(BTreeMap::new());

/// What `id` is bound to, for a label beside the thing it runs. Only the
/// first chord: aliases are real but a row has one slot, and the first is
/// the one the defaults lead with.
pub fn shortcut(id: &str) -> Option<String> {
    SHORTCUTS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(id)
        .cloned()
}

/// Rebuild the keymap from the file. Every edit below ends here.
pub fn apply(cx: &mut App) {
    let overrides = Settings::load().keymap;
    let mut bindings = cx.global::<Foreign>().0.clone();
    let mut shortcuts = BTreeMap::new();
    for command in COMMANDS.iter() {
        let chords = chords(command, &overrides);
        if let Some(chord) = chords.first() {
            shortcuts.insert(command.id, display(chord));
        }
        bindings.extend(chords.iter().filter_map(|chord| command.binding(chord)));
    }
    *SHORTCUTS.write().unwrap_or_else(PoisonError::into_inner) = shortcuts;
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// Give `id` another chord on top of what it already has. A chord the
/// command already holds is dropped, so pressing the same keys twice
/// doesn't bind it twice.
pub fn add(id: &str, chord: String, cx: &mut App) {
    edit(id, cx, move |chords| {
        if !chords.contains(&chord) {
            chords.push(chord);
        }
    });
}

/// Take one chord off `id`, leaving the rest. Taking the last one leaves
/// the command bound to nothing, which is a state the file records.
pub fn remove(id: &str, chord: &str, cx: &mut App) {
    let chord = chord.to_string();
    edit(id, cx, move |chords| chords.retain(|held| *held != chord));
}

/// Put `id` back on the chords it ships with.
pub fn reset(id: &str, cx: &mut App) {
    let id = id.to_string();
    Settings::update(move |settings| {
        settings.keymap.remove(&id);
    });
    apply(cx);
}

/// Put every command back, including any the registry no longer knows:
/// this is the page's escape hatch, so it clears the whole map rather
/// than the rows that happen to be on screen.
pub fn reset_all(cx: &mut App) {
    Settings::update(|settings| settings.keymap.clear());
    apply(cx);
}

/// Put a whole override map back, the undo for a reset: the page snapshots
/// the map before it clears, and this writes the snapshot over whatever
/// the file holds now.
pub fn restore(map: BTreeMap<String, Vec<String>>, cx: &mut App) {
    Settings::update(move |settings| settings.keymap = map);
    apply(cx);
}

/// Read a command's chords, change them, write them back, rebind. The
/// read seeds from the defaults when the file has nothing yet, so the
/// first edit to a command keeps its other chords instead of dropping
/// them.
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

/// A chord as a person reads it: "ctrl-shift-s" comes back "Ctrl+Shift+S".
/// The parts of a sequence stay separated by a space, the way gpui writes
/// them and the way a chord like "g g" has to read.
pub fn display(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(|key| match Keystroke::parse(key) {
            Ok(keystroke) => display_keystroke(&keystroke),
            // Only reachable through a hand-edited file, where showing the
            // raw text is more use than showing nothing.
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

/// One key's printed name. Single characters go up so `s` reads as the
/// cap it's printed on; the named keys get their usual spelling, with the
/// function row left uppercase whole.
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

    /// Two commands sharing a chord in one scope means one of them can
    /// never fire, so the shipped set must not have any.
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

    /// gpui takes a bare modifier as a binding on that modifier's tap,
    /// and takes an unknown word as a key that never fires. Both
    /// are things a hand-edited file can hold, and neither should cost
    /// the file the rest of its bindings.
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

    /// The first keystroke of a sequence is the one a focused input would
    /// eat, so it's the one that decides.
    #[test]
    fn sequences_read_their_opening_chord() {
        assert!(modified("ctrl-k left"));
        assert!(!modified("g ctrl-f"));
    }

    /// The point of the split: a playback command left on its bare default
    /// keeps handing the key back to the search box, and the same command
    /// rebound onto a modified chord fires while you type.
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

    /// Rebinding a playback command onto a chord the workspace already
    /// holds is a real collision once it widens, so the page has to say so
    /// rather than let the loser bind and never fire.
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

    /// Widening is scoped to the carved-out workspace scopes. A command
    /// already on the plain workspace scope, or on the lyrics editor's,
    /// stays put no matter what it's bound to.
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
