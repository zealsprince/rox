//! Every chord rox binds, in one list, so the Keymap settings page has
//! something to draw and the settings file has something to override.
//!
//! Bindings used to be a `bind_keys` call at startup with the platform
//! forks inline. They still are, except the call is built from
//! [`COMMANDS`] rather than written out, and each command's chords come
//! from the settings file when it has an opinion and from the command's
//! own defaults when it doesn't. That split is the whole design: the file
//! only ever holds what someone changed, so a default that moves in a
//! later build reaches everyone who left it alone.
//!
//! Rebinding at runtime means rebuilding the keymap, because gpui only
//! offers add and clear - there's no remove. Clearing takes the widget
//! library's bindings with it (every text input's editing keys live in
//! there), so [`init`] snapshots what was already registered before rox
//! adds its own, and every rebuild lays that snapshot back down first.
//! Anything binding keys after [`init`] runs would be lost on the first
//! rebind; today nothing does, and this is the note explaining why the
//! init order in `main` matters.

use std::collections::BTreeMap;

use gpui::{App, Global, KeyBinding, Keystroke};

use rox_core::settings::Settings;

use rox_dock::ToggleZoom;
use rox_panel_api::actions::{SeekBackward, SeekForward, TogglePlayback};
use rox_panels::lyrics::StampLine;

use crate::workspace::{
    CloseWindow, DecreaseFontSize, FocusSearch, IncreaseFontSize, OpenQuickPlay, OpenSettings,
    OpenStats, Quit, ResetFontSize, TogglePostShader,
};

/// Bindings match key contexts along the focus path, so this scope holds
/// anywhere inside a workspace window except while the library search box
/// is focused: there space and arrows keep typing into the query.
/// Bindings win over key listeners, so the exclusion is what hands the
/// keys back.
const PLAYBACK: Option<&str> = Some("Workspace && !SearchInput");

/// The plain workspace scope: anywhere in a workspace window, the search
/// box included, since everything bound here carries a modifier.
const WORKSPACE: Option<&str> = Some("Workspace");

/// The lyrics editor's own scope, deeper along the focus path than the
/// window root.
const LYRICS: Option<&str> = Some("LyricsEdit");

/// Which part of the app a command belongs to. The Keymap page draws one
/// section per group, in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Playback,
    Windows,
    View,
    Editing,
}

impl Group {
    /// The groups the page walks, in the order it draws them.
    pub const ALL: &'static [Group] =
        &[Group::Playback, Group::Windows, Group::View, Group::Editing];

    pub fn label(self) -> &'static str {
        match self {
            Group::Playback => "Playback",
            Group::Windows => "Windows",
            Group::View => "View",
            Group::Editing => "Editing",
        }
    }

    pub fn icon(self) -> &'static str {
        use rox_design::assets::icons;
        match self {
            Group::Playback => icons::PLAY,
            Group::Windows => icons::APP_WINDOW,
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
    /// The chords this command carries out of the box, in gpui's own
    /// syntax ("ctrl-shift-s"). More than one is an alias, not a
    /// sequence.
    pub defaults: &'static [&'static str],
    /// Builds the binding for one chord. Each command names a distinct
    /// action type, so the type has to be baked in here rather than
    /// carried as data.
    build: fn(&str, Option<&'static str>) -> KeyBinding,
}

impl Command {
    /// The binding for one chord, or `None` when the chord doesn't parse.
    /// A file edited by hand is the way that happens, and dropping the
    /// one bad line beats refusing to bind anything.
    fn binding(&self, chord: &str) -> Option<KeyBinding> {
        parses(chord).then(|| (self.build)(chord, self.context))
    }
}

macro_rules! command {
    ($id:literal, $label:literal, $group:expr, $ctx:expr, $keys:expr, $action:expr, $desc:literal) => {
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
// as one line. macOS puts app-level chords on Cmd; everywhere else they
// sit on Ctrl.
#[cfg(target_os = "macos")]
mod defaults {
    pub const SETTINGS: &[&str] = &["cmd-,", "ctrl-i"];
    pub const STATS: &[&str] = &["cmd-shift-s"];
    pub const QUICK_PLAY: &[&str] = &["cmd-p", "cmd-f"];
    pub const FOCUS_SEARCH: &[&str] = &["cmd-l"];
    pub const ZOOM_IN: &[&str] = &["cmd-=", "cmd-+"];
    pub const ZOOM_OUT: &[&str] = &["cmd--"];
    pub const ZOOM_RESET: &[&str] = &["cmd-0"];
    pub const POST_SHADER: &[&str] = &["cmd-shift-x"];
    pub const CLOSE_WINDOW: &[&str] = &["cmd-w"];
    pub const QUIT: &[&str] = &["cmd-q"];
}

#[cfg(not(target_os = "macos"))]
mod defaults {
    pub const SETTINGS: &[&str] = &["ctrl-,", "ctrl-i"];
    pub const STATS: &[&str] = &["ctrl-shift-s"];
    pub const QUICK_PLAY: &[&str] = &["ctrl-p", "ctrl-f"];
    pub const FOCUS_SEARCH: &[&str] = &["ctrl-l"];
    pub const ZOOM_IN: &[&str] = &["ctrl-=", "ctrl-+"];
    pub const ZOOM_OUT: &[&str] = &["ctrl--"];
    pub const ZOOM_RESET: &[&str] = &["ctrl-0"];
    pub const POST_SHADER: &[&str] = &["ctrl-shift-x"];
    pub const CLOSE_WINDOW: &[&str] = &["ctrl-w"];
    pub const QUIT: &[&str] = &["alt-f4"];
}

/// Everything rox binds. The page draws this in order within each group,
/// so related rows sit together.
pub const COMMANDS: &[Command] = &[
    command!(
        "toggle_playback",
        "Play / Pause",
        Group::Playback,
        PLAYBACK,
        &["space"],
        TogglePlayback,
        "Start the current track, or pause it where it is"
    ),
    command!(
        "seek_backward",
        "Seek Backward",
        Group::Playback,
        PLAYBACK,
        &["left"],
        SeekBackward,
        "Step back through the playing track"
    ),
    command!(
        "seek_forward",
        "Seek Forward",
        Group::Playback,
        PLAYBACK,
        &["right"],
        SeekForward,
        "Step forward through the playing track"
    ),
    command!(
        "open_settings",
        "Open Settings",
        Group::Windows,
        WORKSPACE,
        defaults::SETTINGS,
        OpenSettings,
        "Open this window"
    ),
    command!(
        "open_stats",
        "Open Statistics",
        Group::Windows,
        WORKSPACE,
        defaults::STATS,
        OpenStats,
        "Open the listening statistics window"
    ),
    command!(
        "open_quick_play",
        "Quick Play",
        Group::Windows,
        WORKSPACE,
        defaults::QUICK_PLAY,
        OpenQuickPlay,
        "Raise the search-and-play prompt over the window"
    ),
    command!(
        "close_window",
        "Close Window",
        Group::Windows,
        None,
        defaults::CLOSE_WINDOW,
        CloseWindow,
        "Close whichever window is in front. Bound everywhere, popped-out panels included"
    ),
    command!(
        "quit",
        "Quit",
        Group::Windows,
        None,
        defaults::QUIT,
        Quit,
        "Leave rox. Bound everywhere, since there's no window it shouldn't work from"
    ),
    command!(
        "focus_search",
        "Focus Search",
        Group::View,
        WORKSPACE,
        defaults::FOCUS_SEARCH,
        FocusSearch,
        "Put the cursor in the library search box"
    ),
    command!(
        "toggle_zoom",
        "Zoom Panel Group",
        Group::View,
        WORKSPACE,
        &["shift-escape"],
        ToggleZoom,
        "Fill the dock with the last-clicked panel group, or back out of it"
    ),
    command!(
        "increase_font_size",
        "Increase Text Size",
        Group::View,
        None,
        defaults::ZOOM_IN,
        IncreaseFontSize,
        "Step the app-wide text size up"
    ),
    command!(
        "decrease_font_size",
        "Decrease Text Size",
        Group::View,
        None,
        defaults::ZOOM_OUT,
        DecreaseFontSize,
        "Step the app-wide text size down"
    ),
    command!(
        "reset_font_size",
        "Reset Text Size",
        Group::View,
        None,
        defaults::ZOOM_RESET,
        ResetFontSize,
        "Snap the text size back to stock"
    ),
    command!(
        "toggle_post_shader",
        "Toggle Overlay Shader",
        Group::View,
        None,
        defaults::POST_SHADER,
        TogglePostShader,
        "Turn the screen shader off and on. Bound everywhere on purpose: a shader can bury \
         every control this chord would otherwise be reached by"
    ),
    command!(
        "stamp_line",
        "Stamp Lyric Line",
        Group::Editing,
        LYRICS,
        &["shift-enter"],
        StampLine,
        "Write the playing position onto the lyric line being edited"
    ),
];

/// The command with this id, if the registry still has one. A settings
/// file written by an older or newer build can name commands this one
/// doesn't know; those entries stay in the file untouched and are simply
/// not bound.
pub fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// Whether a chord is one gpui can bind. Whitespace separates the
/// keystrokes of a sequence, so every part has to parse on its own.
///
/// This is a shape check, and a loose one, because gpui's own parse is
/// loose: any word that isn't a modifier is taken as a key name, so
/// "ctrl-nonsense" binds cleanly and simply never fires, and a bare
/// "ctrl" is a real binding on a modifier tap. What's left to reject is
/// the empty chord, which is what an emptied field in a hand-edited file
/// leaves behind.
pub fn parses(chord: &str) -> bool {
    let mut keystrokes = chord.split_whitespace().peekable();
    keystrokes.peek().is_some() && keystrokes.all(|key| Keystroke::parse(key).is_ok())
}

/// The chords a command is running: what the file says when it says
/// anything, the command's own defaults when it doesn't.
pub fn chords(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    match overrides.get(command.id) {
        Some(chords) => chords.clone(),
        None => command.defaults.iter().map(|s| s.to_string()).collect(),
    }
}

/// Whether a command still carries exactly what it ships with, which is
/// what decides if the page offers a reset.
pub fn is_default(command: &Command, overrides: &BTreeMap<String, Vec<String>>) -> bool {
    match overrides.get(command.id) {
        Some(chords) => chords.as_slice() == command.defaults,
        None => true,
    }
}

/// Another command holding the same chord somewhere this one is also
/// live, if there is one. This compares context strings rather than
/// working out whether two predicates can both be true at once: an
/// unscoped binding is live everywhere so it clashes with anything, and
/// two scoped ones only count as clashing when they name the same scope.
/// That misses a pair whose predicates overlap without matching, which
/// today can't happen - every scope here is a distinct window or editor.
pub fn clash(
    command: &Command,
    chord: &str,
    overrides: &BTreeMap<String, Vec<String>>,
) -> Option<&'static str> {
    COMMANDS
        .iter()
        .filter(|other| other.id != command.id)
        .filter(|other| {
            other.context.is_none() || command.context.is_none() || other.context == command.context
        })
        .find(|other| chords(other, overrides).iter().any(|held| held == chord))
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

/// Rebuild the keymap from the file. Every edit below ends here.
pub fn apply(cx: &mut App) {
    let overrides = Settings::load().keymap;
    let mut bindings = cx.global::<Foreign>().0.clone();
    for command in COMMANDS {
        bindings.extend(
            chords(command, &overrides)
                .iter()
                .filter_map(|chord| command.binding(chord)),
        );
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

/// Give `id` another chord on top of what it already carries. A chord the
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

/// Read a command's chords, change them, write them back, rebind. The
/// read seeds from the defaults when the file has nothing yet, so the
/// first edit to a command carries its other chords along instead of
/// dropping them.
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
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push(if cfg!(target_os = "macos") {
            "Option"
        } else {
            "Alt"
        });
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.platform {
        parts.push(match () {
            _ if cfg!(target_os = "macos") => "Cmd",
            _ if cfg!(target_os = "windows") => "Win",
            _ => "Super",
        });
    }
    if modifiers.function {
        parts.push("Fn");
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
        "escape" => "Esc".to_string(),
        "enter" => "Enter".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "pageup" => "Page Up".to_string(),
        "pagedown" => "Page Down".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "insert" => "Insert".to_string(),
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
        for command in COMMANDS {
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
        for command in COMMANDS {
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
        assert_eq!(display("ctrl-shift-s"), "Ctrl+Shift+S");
        assert_eq!(display("space"), "Space");
        assert_eq!(display("shift-escape"), "Shift+Esc");
    }

    #[test]
    fn empty_chords_do_not_parse() {
        assert!(!parses(""));
        assert!(!parses("   "));
    }

    /// gpui takes a bare modifier as a binding on that modifier's tap,
    /// and takes an unknown word as a key that simply never fires. Both
    /// are things a hand-edited file can hold, and neither should cost
    /// the file the rest of its bindings.
    #[test]
    fn loose_chords_still_bind() {
        assert!(parses("ctrl"));
        assert!(parses("ctrl-nonsense"));
        assert!(parses("ctrl-k ctrl-s"), "a two-keystroke sequence");
    }
}
