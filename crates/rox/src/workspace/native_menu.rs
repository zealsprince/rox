//! The macOS system menu bar, built from the same [`MENUS`] table the
//! in-window bar renders. gpui only has `set_menus` on macOS, so this is a
//! no-op elsewhere.
//!
//! The native bar is a snapshot AppKit never re-reads, so labels and saved
//! lists are baked in at [`rebuild`] time, and everything that changes one
//! calls [`rebuild`] again.

use super::*;

/// Pushed in by the player through [`sync_playback`]: most rebuilds run
/// inside a workspace update, where reading the player's workspace panics.
#[cfg(target_os = "macos")]
static MENU_PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Rebuilds only on a flip; the player notifies every pump tick.
#[cfg(target_os = "macos")]
pub(crate) fn sync_playback(playing: bool, cx: &mut App) {
    if MENU_PLAYING.swap(playing, std::sync::atomic::Ordering::Relaxed) != playing {
        rebuild(cx);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn sync_playback(_playing: bool, _cx: &mut App) {}

/// Reads no entities, so it's safe inside any update.
#[cfg(target_os = "macos")]
pub(crate) fn rebuild(cx: &mut App) {
    let playing = MENU_PLAYING.load(std::sync::atomic::Ordering::Relaxed);
    let menus = MENUS
        .iter()
        .map(|menu| gpui::Menu {
            name: rox_i18n::t!(menu.label),
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

#[cfg(target_os = "macos")]
fn entry_items(entry: &'static MenuEntry, playing: bool) -> Vec<gpui::MenuItem> {
    match entry {
        MenuEntry::Item(item) => action_item(*item, playing).into_iter().collect(),
        MenuEntry::Section(_) => vec![gpui::MenuItem::separator()],
        MenuEntry::Panels(section) if !crate::workspace::section_shows(section) => Vec::new(),
        MenuEntry::Panels(section) => match section.group {
            None => section
                .panels
                .iter()
                .filter_map(|def| action_item(panel_menu_item(def), playing))
                .collect(),
            Some((label, _)) => vec![gpui::MenuItem::submenu(gpui::Menu {
                name: rox_i18n::t!(label),
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
            name: rox_i18n::t!(*label),
            items: layout_items(*target, *with_new),
        })],
        MenuEntry::WorkspacesSubmenu {
            label,
            target,
            with_new,
            ..
        } => vec![gpui::MenuItem::submenu(gpui::Menu {
            name: rox_i18n::t!(*label),
            items: workspace_items(*target, *with_new),
        })],
        MenuEntry::PresetsSubmenu { label, target, .. } => {
            vec![gpui::MenuItem::submenu(gpui::Menu {
                name: rox_i18n::t!(*label),
                items: preset_items(*target),
            })]
        }
        MenuEntry::PanelWindowsSubmenu { label, .. } => {
            vec![gpui::MenuItem::submenu(gpui::Menu {
                name: rox_i18n::t!(*label),
                items: panel_window_items(),
            })]
        }
    }
}

/// The rows with a keybinding emit their own action so AppKit draws the
/// shortcut; the rest go through the [`MenuCommand`] bridge.
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

/// Native items have no checkmark, so toggle rows name what picking them
/// does.
#[cfg(target_os = "macos")]
fn native_label(item: MenuItem, playing: bool) -> String {
    match item.action {
        MenuAction::TogglePlayback => {
            if playing {
                rox_i18n::t!("menu-pause").to_string()
            } else {
                rox_i18n::t!("playback-item-play").to_string()
            }
        }
        MenuAction::ToggleMenubar => showing(!settings::hide_menubar(), "Menubar"),
        MenuAction::ToggleDecorations => showing(
            settings::os_decorations(),
            rox_i18n::t!("menu-os-decorations"),
        ),
        MenuAction::ToggleDesignMode => {
            switching(settings::design_mode(), rox_i18n::t!("menu-design-mode"))
        }
        MenuAction::ToggleArtTheming => {
            switching(palette::art_theming(), rox_i18n::t!("menu-song-theming"))
        }
        MenuAction::TogglePostShader => switching(
            crate::workspace::post_shader_on(),
            rox_i18n::t!("menu-overlay-shader"),
        ),
        MenuAction::ToggleQuitToTray => switching(
            settings::quit_to_tray(),
            rox_i18n::t!("menu-remain-in-tray"),
        ),
        // `item.label` is an i18n key, not display text.
        _ => rox_i18n::t_static(item.label).into(),
    }
}

#[cfg(target_os = "macos")]
fn showing(on: bool, what: impl std::fmt::Display) -> String {
    if on {
        format!("Hide {what}")
    } else {
        format!("Show {what}")
    }
}

#[cfg(target_os = "macos")]
fn switching(on: bool, what: impl std::fmt::Display) -> String {
    if on {
        format!("Turn Off {what}")
    } else {
        format!("Turn On {what}")
    }
}

#[cfg(target_os = "macos")]
fn layout_items(target: LayoutTarget, with_new: bool) -> Vec<gpui::MenuItem> {
    let kind = match target {
        LayoutTarget::NewWindow => "layout-new",
        LayoutTarget::Overwrite => "layout-save",
        LayoutTarget::Apply => "layout-apply",
    };
    let presets = rox_core::settings::layouts::all(&Settings::load());
    let mut items = Vec::new();
    if with_new {
        items.push(gpui::MenuItem::action(
            rox_i18n::t!("menu-new-ellipsis"),
            MenuCommand {
                command: "layout-save-new".into(),
            },
        ));
    }
    if presets.is_empty() {
        if !with_new {
            items.push(placeholder(rox_i18n::t!("menu-no-layouts")));
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
            rox_i18n::t!("menu-new-ellipsis"),
            MenuCommand {
                command: "workspace-save-new".into(),
            },
        ));
    }
    if entries.is_empty() {
        if !with_new {
            items.push(placeholder(rox_i18n::t!("menu-no-workspaces")));
        }
    } else {
        items.extend(entries.into_iter().map(|entry| {
            let label = if entry.builtin {
                format!(
                    "{} ({})",
                    entry.title,
                    rox_i18n::t!("menu-workspace-builtin-tag")
                )
            } else {
                entry.title.to_string()
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

#[cfg(target_os = "macos")]
fn preset_items(target: PanelTarget) -> Vec<gpui::MenuItem> {
    let kind = match target {
        PanelTarget::Open => "panel-preset",
        PanelTarget::NewWindow => "panel-preset-window",
    };
    let presets = crate::panel_presets::saved();
    if presets.is_empty() {
        return vec![placeholder(rox_i18n::t!("menu-no-presets"))];
    }
    presets
        .into_iter()
        .map(|preset| {
            gpui::MenuItem::action(
                preset.name.clone(),
                MenuCommand {
                    command: format!("{kind}:{}", preset.name),
                },
            )
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn panel_window_items() -> Vec<gpui::MenuItem> {
    let mut items = Vec::new();
    let presets = crate::panel_presets::saved();
    if !presets.is_empty() {
        items.push(gpui::MenuItem::submenu(gpui::Menu {
            name: rox_i18n::t!(crate::panel_presets::GROUP_LABEL),
            items: preset_items(PanelTarget::NewWindow),
        }));
    }
    for section in catalog::sections() {
        let rows: Vec<gpui::MenuItem> = section
            .panels
            .iter()
            .map(|def| {
                gpui::MenuItem::action(
                    rox_i18n::t!(def.label),
                    MenuCommand {
                        command: format!("panel-window:{}", def.name),
                    },
                )
            })
            .collect();
        match section.group {
            None => items.extend(rows),
            Some((label, _)) => items.push(gpui::MenuItem::submenu(gpui::Menu {
                name: rox_i18n::t!(label),
                items: rows,
            })),
        }
    }
    items
}

/// Picking it does nothing. It can't be greyed: enablement goes by action
/// type, and [`MenuCommand`] has a handler.
#[cfg(target_os = "macos")]
fn placeholder(label: impl Into<SharedString>) -> gpui::MenuItem {
    gpui::MenuItem::action(
        label,
        MenuCommand {
            command: "none".into(),
        },
    )
}
