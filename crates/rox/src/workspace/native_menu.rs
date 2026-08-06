//! The macOS system menu bar, built from the same [`MENUS`] table the
//! in-window bar renders. gpui only surfaces `set_menus` on macOS, so this
//! whole module is a no-op elsewhere and the in-window bar stays the menu
//! everywhere else.
//!
//! The native bar is a snapshot, not a live view: gpui hands AppKit a fixed
//! tree and nothing re-reads it. So the toggles' labels, the Play/Pause
//! label, and the saved workspaces and layouts are all baked in at
//! [`rebuild`] time, and everything that changes one of them calls
//! [`rebuild`] again.

use super::*;

/// Whether the Play row should read "Pause", kept here rather than read off
/// the player. Most rebuilds run from inside a workspace update - a menu
/// toggle, the player observer - and reaching the player means reading the
/// workspace that holds it, which panics while that workspace is the one
/// being updated. So the player pushes its state in through
/// [`sync_playback`] and the builder only ever reads this.
#[cfg(target_os = "macos")]
static MENU_PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Note the player's play/pause state, rebuilding only when it actually
/// flips. The player notifies on every pump tick, far too often to rebuild
/// a menu tree for, and the label only has two states.
#[cfg(target_os = "macos")]
pub(crate) fn sync_playback(playing: bool, cx: &mut App) {
    if MENU_PLAYING.swap(playing, std::sync::atomic::Ordering::Relaxed) != playing {
        rebuild(cx);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sync_playback(_playing: bool, _cx: &mut App) {}

/// Rebuild the macOS system menu bar from the current state. Cheap enough to
/// call on any change that touches a label or a saved set - it walks the menu
/// table and allocates a few dozen strings. Reads no entities, so it is safe
/// to call from inside any update.
#[cfg(target_os = "macos")]
pub(crate) fn rebuild(cx: &mut App) {
    let playing = MENU_PLAYING.load(std::sync::atomic::Ordering::Relaxed);
    let menus = MENUS
        .iter()
        .map(|menu| gpui::Menu {
            name: menu.label.into(),
            items: menu
                .entries
                .iter()
                .flat_map(|entry| entry_items(entry, playing))
                .collect(),
        })
        .collect();
    cx.set_menus(menus);
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn rebuild(_cx: &mut App) {}

/// One menu-table entry as native items. A section heading has no native
/// equivalent, so it becomes the separator it already draws as; everything
/// else is one item.
#[cfg(target_os = "macos")]
fn entry_items(entry: &'static MenuEntry, playing: bool) -> Vec<gpui::MenuItem> {
    match entry {
        MenuEntry::Item(item) => action_item(*item, playing).into_iter().collect(),
        MenuEntry::Section(_) => vec![gpui::MenuItem::separator()],
        MenuEntry::Panels(section) => match section.group {
            // A bare section is a run of rows in place, same as in-window.
            None => section
                .panels
                .iter()
                .filter_map(|def| action_item(panel_menu_item(def), playing))
                .collect(),
            Some((label, _)) => vec![gpui::MenuItem::submenu(gpui::Menu {
                name: label.into(),
                items: section
                    .panels
                    .iter()
                    .filter_map(|def| action_item(panel_menu_item(def), playing))
                    .collect(),
            })],
        },
        MenuEntry::LayoutsSubmenu {
            label,
            target,
            with_new,
            ..
        } => vec![gpui::MenuItem::submenu(gpui::Menu {
            name: (*label).into(),
            items: layout_items(*target, *with_new),
        })],
        MenuEntry::WorkspacesSubmenu {
            label,
            target,
            with_new,
            ..
        } => vec![gpui::MenuItem::submenu(gpui::Menu {
            name: (*label).into(),
            items: workspace_items(*target, *with_new),
        })],
    }
}

/// A plain row as a native item. The four rows that already carry a
/// keybinding emit their own action, so AppKit draws the shortcut beside
/// them; everything else rides the [`MenuCommand`] bridge.
#[cfg(target_os = "macos")]
fn action_item(item: MenuItem, playing: bool) -> Option<gpui::MenuItem> {
    let label = native_label(item, playing);
    let native = match item.action {
        MenuAction::TogglePlayback => gpui::MenuItem::action(label, TogglePlayback),
        MenuAction::OpenSettings => gpui::MenuItem::action(label, OpenSettings),
        MenuAction::OpenStats => gpui::MenuItem::action(label, OpenStats),
        MenuAction::Quit => gpui::MenuItem::action(label, Quit),
        action => gpui::MenuItem::action(
            label,
            MenuCommand {
                command: action.command_id()?,
            },
        ),
    };
    Some(native)
}

/// The label a native row shows. There is no checkmark on a native menu
/// item, so the toggle rows name what picking them does rather than what
/// they are, and Play/Pause follows the player like it does in-window.
#[cfg(target_os = "macos")]
fn native_label(item: MenuItem, playing: bool) -> String {
    match item.action {
        MenuAction::TogglePlayback => {
            if playing {
                "Pause".into()
            } else {
                "Play".into()
            }
        }
        // The two that hide something take Show/Hide; the two that switch a
        // behavior on and off take Turn On/Off, which is what each one
        // actually does to the thing it names.
        MenuAction::ToggleMenubar => showing(!settings::hide_menubar(), "Menubar"),
        MenuAction::ToggleDecorations => showing(settings::os_decorations(), "OS Decorations"),
        MenuAction::ToggleArtTheming => switching(palette::art_theming(), "Song Theming"),
        MenuAction::TogglePostShader => {
            switching(crate::workspace::post_shader_on(), "Screen Shader")
        }
        MenuAction::ToggleQuitToTray => switching(settings::quit_to_tray(), "Remain in Tray"),
        _ => item.label.into(),
    }
}

/// A toggle row for something visible: "Hide X" while X shows.
#[cfg(target_os = "macos")]
fn showing(on: bool, what: &str) -> String {
    if on {
        format!("Hide {what}")
    } else {
        format!("Show {what}")
    }
}

/// A toggle row for a behavior: "Turn Off X" while X is on.
#[cfg(target_os = "macos")]
fn switching(on: bool, what: &str) -> String {
    if on {
        format!("Turn Off {what}")
    } else {
        format!("Turn On {what}")
    }
}

/// The saved layouts as native rows, read now because the native bar can't
/// read them later. Mirrors the in-window flyout: an optional "New..." lead
/// row, then the presets, or a disabled-looking placeholder when there are
/// none and no New row to carry the flyout.
#[cfg(target_os = "macos")]
fn layout_items(target: LayoutTarget, with_new: bool) -> Vec<gpui::MenuItem> {
    let kind = match target {
        LayoutTarget::NewWindow => "layout-new",
        LayoutTarget::Overwrite => "layout-save",
        LayoutTarget::Apply => "layout-apply",
    };
    let presets = crate::settings::layouts::all(&Settings::load());
    let mut items = Vec::new();
    if with_new {
        items.push(gpui::MenuItem::action(
            "New...",
            MenuCommand {
                command: "layout-save-new".into(),
            },
        ));
    }
    if presets.is_empty() {
        if !with_new {
            items.push(placeholder("No layouts"));
        }
    } else {
        items.extend(presets.into_iter().map(|preset| {
            gpui::MenuItem::action(
                preset.name.clone(),
                MenuCommand {
                    command: format!("{kind}:{}", preset.name),
                },
            )
        }));
    }
    items
}

/// The saved and shipped workspaces as native rows. Same shape as
/// [`layout_items`]; the Save flyout drops the shipped bundles, which it
/// can't overwrite, exactly like the in-window flyout does.
#[cfg(target_os = "macos")]
fn workspace_items(target: WorkspaceTarget, with_new: bool) -> Vec<gpui::MenuItem> {
    let kind = match target {
        WorkspaceTarget::Apply => "workspace-apply",
        WorkspaceTarget::Overwrite => "workspace-save",
    };
    let mut entries = crate::workspaces::all();
    if target == WorkspaceTarget::Overwrite {
        entries.retain(|entry| !entry.builtin);
    }
    let mut items = Vec::new();
    if with_new {
        items.push(gpui::MenuItem::action(
            "New...",
            MenuCommand {
                command: "workspace-save-new".into(),
            },
        ));
    }
    if entries.is_empty() {
        if !with_new {
            items.push(placeholder("No workspaces"));
        }
    } else {
        items.extend(entries.into_iter().map(|entry| {
            // The shipped bundles trail the same tag the in-window rows show.
            let label = if entry.builtin {
                format!("{} (Built-in)", entry.name)
            } else {
                entry.name.clone()
            };
            gpui::MenuItem::action(
                label,
                MenuCommand {
                    command: format!("{kind}:{}", entry.name),
                },
            )
        }));
    }
    items
}

/// An empty flyout's one row, matching the in-window "No layouts" text. It
/// carries a command that decodes to nothing, so picking it does nothing.
/// It reads enabled rather than greyed: enablement goes by action type and
/// [`MenuCommand`] has a handler, so there's no way to disable one row.
#[cfg(target_os = "macos")]
fn placeholder(label: &str) -> gpui::MenuItem {
    gpui::MenuItem::action(
        label.to_string(),
        MenuCommand {
            command: "none".into(),
        },
    )
}
