//! Panel presets: one saved panel, rebuilt on demand. A preset is the same
//! leaf a layout dump stores per panel, so building one is the restore path
//! a layout takes for that node: the registry name routes through
//! [`PanelRegistry`], the config blob goes back in, and a composite's
//! children come along.
//!
//! Saving happens down in `rox_panel_api::panel_settings` (the dropdown that
//! owns the panel); everything that turns a saved preset back into a live
//! panel is here, because only this crate has the catalog.

use std::sync::Arc;

use gpui::{App, Context, WeakEntity, Window};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;

use rox_core::settings::{PanelPreset, Settings};
use rox_design::assets::icons;
use rox_dock::{DockArea, PanelInfo, PanelRegistry, PanelState, PanelView};

use crate::panel_catalog::{self as catalog, PanelPlacement};

/// What the presets group is called and the icon it shows wherever a panel
/// picker lists it, so the group reads the same in every menu that has one.
pub(crate) const GROUP_LABEL: &str = "Presets";
pub(crate) const GROUP_ICON: &str = icons::COPY;

/// Every preset the live look holds, in save order. Read at the moment a
/// menu opens rather than held, the way the layout flyouts read theirs.
pub(crate) fn saved() -> Vec<PanelPreset> {
    rox_core::settings::panel_presets::all(&Settings::load())
}

/// The icon a preset's row shows: the icon of the panel inside it, so a
/// preset reads as the thing it makes. A preset whose panel isn't in the
/// catalog falls back to the group's own glyph.
pub(crate) fn icon_for(preset: &PanelPreset) -> &'static str {
    preset
        .panel_name()
        .and_then(catalog::def_for)
        .map(|def| def.icon)
        .unwrap_or(GROUP_ICON)
}

/// Where a preset's panel joins the layout when it's opened from a menu with
/// no group under the pointer: its catalog entry's placement, center for a
/// panel the catalog doesn't list.
pub(crate) fn placement_for(preset: &PanelPreset) -> PanelPlacement {
    preset
        .panel_name()
        .and_then(catalog::def_for)
        .map(|def| def.placement)
        .unwrap_or(PanelPlacement::Center)
}

/// Whether a preset holds a composition host, which the slot pickers gray
/// out: a composite can go in a tab but not in another composite's slot,
/// and a preset of one is still one.
pub(crate) fn is_arrangement(preset: &PanelPreset) -> bool {
    preset
        .panel_name()
        .and_then(catalog::section_for)
        .is_some_and(catalog::is_arrangement)
}

/// Build the panel a preset holds, against the registry the workspace
/// owning `dock` registered. None when the stored dump won't parse; a name
/// nothing registers comes back as the dock's invalid-panel placeholder,
/// the same as a layout holding a panel this build doesn't have.
pub(crate) fn build(
    preset: &PanelPreset,
    dock: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<dyn PanelView>> {
    let state: PanelState = match serde_json::from_value(preset.panel.clone()) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("panel presets: {} did not parse: {e}", preset.name);
            return None;
        }
    };
    let info = state.info.clone();
    // Only a leaf makes sense as a preset: the containers are the dock's own
    // nodes, and one saved alone would rebuild as an empty tab strip.
    if !matches!(info, PanelInfo::Panel(_)) {
        log::warn!(
            "panel presets: {} holds a container, not a panel",
            preset.name
        );
        return None;
    }
    Some(PanelRegistry::build_panel(&state.panel_name, dock, &state, &info, window, cx).into())
}

/// Build the preset named `name`, or nothing when the look has since dropped
/// it. The flyouts hold names rather than dumps, so a preset deleted while a
/// menu stood open picks as a no-op instead of stale settings.
pub(crate) fn build_named(
    name: &str,
    dock: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<dyn PanelView>> {
    let preset = rox_core::settings::panel_presets::resolve(&Settings::load(), name)?;
    build(&preset, dock, window, cx)
}

/// Lead a panel picker with the Presets group: one flyout of the saved
/// panels above the catalog's own groups, skipped whole when nothing is
/// saved. A pick builds the preset and hands it to `on_pick`, which decides
/// where it goes, the same split [`crate::composite::pick_items`] draws.
///
/// `no_composites` grays the presets that hold a composition host, for the
/// slot pickers that can't take one.
pub(crate) fn pick_submenu(
    menu: PopupMenu,
    dock: WeakEntity<DockArea>,
    no_composites: bool,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    on_pick: impl Fn(Arc<dyn PanelView>, &mut Window, &mut App) + Clone + 'static,
) -> PopupMenu {
    let presets = saved();
    if presets.is_empty() {
        return menu;
    }
    menu.submenu_with_icon(
        Some(Icon::default().path(GROUP_ICON)),
        GROUP_LABEL,
        window,
        cx,
        move |mut menu, _, _| {
            for preset in &presets {
                let disabled = no_composites && is_arrangement(preset);
                let item = PopupMenuItem::new(preset.name.clone())
                    .icon(Icon::default().path(icon_for(preset)));
                if disabled {
                    menu = menu.item(item.disabled(true));
                    continue;
                }
                let name = preset.name.clone();
                let dock = dock.clone();
                let on_pick = on_pick.clone();
                menu = menu.item(item.on_click(move |_, window, cx| {
                    if let Some(panel) = build_named(&name, dock.clone(), window, cx) {
                        on_pick(panel, window, cx);
                    }
                }));
            }
            menu
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump makes the round trip a save and an add put it through: a panel
    /// state to JSON, the kind readable off it without parsing, and back to
    /// the state the registry builds from. The two halves are in different
    /// crates, so nothing else checks they agree on the shape.
    #[test]
    fn a_dump_round_trips_through_a_preset() {
        let dump = PanelState {
            panel_name: "spectrum".into(),
            children: Vec::new(),
            info: PanelInfo::Panel(serde_json::json!({ "bars": 64 })),
        };
        let preset = PanelPreset {
            name: "Scope".into(),
            panel: serde_json::to_value(&dump).expect("a dump serializes"),
        };
        assert_eq!(preset.panel_name(), Some("spectrum"));
        // The catalog resolves what that name is and where it goes.
        assert_eq!(icon_for(&preset), icons::AUDIO_LINES);
        assert!(matches!(placement_for(&preset), PanelPlacement::Bottom));
        assert!(!is_arrangement(&preset));

        let back: PanelState = serde_json::from_value(preset.panel).expect("and parses back");
        assert_eq!(back.panel_name, "spectrum");
        assert_eq!(
            back.info,
            PanelInfo::Panel(serde_json::json!({ "bars": 64 }))
        );
    }

    /// A preset of a composition host reads as one, which keeps it out of
    /// another composite's slot.
    #[test]
    fn a_composite_preset_reads_as_one() {
        let preset = PanelPreset {
            name: "Split".into(),
            panel: serde_json::json!({ "panel_name": "group" }),
        };
        assert!(is_arrangement(&preset));
    }
}
