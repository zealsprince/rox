//! Panel presets: one saved panel, rebuilt through the same restore path a
//! layout takes for that node. Saving lives in `rox_panel_api::panel_settings`;
//! rebuilding is here because only this crate has the catalog.

use std::sync::Arc;

use gpui::{App, Context, WeakEntity, Window};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};

use rox_core::settings::{PanelPreset, Settings};
use rox_design::assets::icons;
use rox_dock::{DockArea, PanelInfo, PanelRegistry, PanelState, PanelView};

use crate::panel_catalog::{self as catalog, PanelPlacement};

/// The presets group's i18n label and icon, shared by every panel picker.
pub(crate) const GROUP_LABEL: &str = "menu-panels-presets";
pub(crate) const GROUP_ICON: &str = icons::COPY;

pub(crate) fn saved() -> Vec<PanelPreset> {
    rox_core::settings::panel_presets::all(&Settings::load())
}

pub(crate) fn icon_for(preset: &PanelPreset) -> &'static str {
    preset
        .panel_name()
        .and_then(catalog::def_for)
        .map(|def| def.icon)
        .unwrap_or(GROUP_ICON)
}

pub(crate) fn placement_for(preset: &PanelPreset) -> PanelPlacement {
    preset
        .panel_name()
        .and_then(catalog::def_for)
        .map(|def| def.placement)
        .unwrap_or(PanelPlacement::Center)
}

/// The slot pickers gray these out: a composite can't nest in another.
pub(crate) fn is_arrangement(preset: &PanelPreset) -> bool {
    preset
        .panel_name()
        .and_then(catalog::section_for)
        .is_some_and(catalog::is_arrangement)
}

/// None when the dump won't parse. An unregistered name builds the dock's
/// invalid-panel placeholder, like a layout would.
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
    // A container saved alone would rebuild as an empty tab strip.
    if !matches!(info, PanelInfo::Panel(_)) {
        log::warn!(
            "panel presets: {} holds a container, not a panel",
            preset.name
        );
        return None;
    }
    Some(PanelRegistry::build_panel(&state.panel_name, dock, &state, &info, window, cx).into())
}

/// A preset deleted while its menu stood open picks as a no-op.
pub(crate) fn build_named(
    name: &str,
    dock: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<dyn PanelView>> {
    let preset = rox_core::settings::panel_presets::resolve(&Settings::load(), name)?;
    build(&preset, dock, window, cx)
}

/// The Presets flyout leading a panel picker, skipped when nothing is saved.
/// `on_pick` decides where the built panel goes.
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
        rox_i18n::t!(GROUP_LABEL),
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

    #[test]
    fn the_group_label_is_a_message_key() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some(rox_i18n::SOURCE_LOCALE));
        assert!(rox_i18n::try_translate(GROUP_LABEL).is_some());
    }

    /// The save and restore halves live in different crates; this pins the shape.
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

    #[test]
    fn a_composite_preset_reads_as_one() {
        let preset = PanelPreset {
            name: "Split".into(),
            panel: serde_json::json!({ "panel_name": "group" }),
        };
        assert!(is_arrangement(&preset));
    }
}
