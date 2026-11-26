//! The controls for a renderer's own settings (ADR 37, amended 2026-11-26),
//! built from what its manifest declares. Both hosts of a render target draw
//! them, the canvas inspector and the Renderer panel's settings, and each
//! stores what the user picks with its target. A row's title and a choice's
//! label are the package's words, shown as written: rox can't translate a
//! stranger's text, and the manifest check holds them to plain text of a
//! size these rows can show.
//!
//! Nothing here sends anything. The value lands in the target's store, and
//! the target's session holds it to the declaration again before the
//! renderer hears it.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use gpui::{AnyElement, Context, SharedString, prelude::*};
use rox_canvas::ImageFit;
use rox_panel_api::panel::{ScrubState, ValueEdit};
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{choices_shared, picker, toggle};
use rox_plugins::renderer_settings::{self, RendererSetting, SettingKind};
use serde_json::Value;

use crate::canvas::target::{SCALES, renderer_id};

/// Past this many choices an enum is a dropdown rather than a row of chips.
const CHIPS_MAX: usize = 4;

/// One setting's row: its title and its control.
pub(crate) struct SettingRow {
    pub(crate) title: SharedString,
    pub(crate) control: AnyElement,
}

thread_local! {
    /// Ids and undo keys made from a setting's key, which gpui and the undo
    /// history take as `&'static str`. One per key ever shown, so bounded by
    /// the keys installed renderers declare.
    static NAMES: RefCell<HashMap<String, &'static str>> = RefCell::new(HashMap::new());
}

/// `prefix` and `key` as one `&'static str`, made once.
pub(crate) fn named(prefix: &str, key: &str) -> &'static str {
    let name = format!("{prefix}{key}");
    NAMES.with(|names| {
        *names
            .borrow_mut()
            .entry(name.clone())
            .or_insert_with(|| Box::leak(name.into_boxed_str()))
    })
}

/// The render scales, worded as percents.
pub(crate) fn scale_choices() -> Vec<(SharedString, u32)> {
    SCALES
        .iter()
        .map(|&percent| {
            let label = rox_i18n::t!(
                "unit-percent",
                value = rox_i18n::format::format_float(f64::from(percent), 0)
            );
            (label, percent)
        })
        .collect()
}

/// The three fits, worded as the image item words them.
pub(crate) fn fit_choices() -> [(SharedString, ImageFit); 3] {
    [
        (rox_i18n::t!("canvas-image-fit-contain"), ImageFit::Contain),
        (rox_i18n::t!("canvas-image-fit-cover"), ImageFit::Cover),
        (rox_i18n::t!("canvas-image-fit-stretch"), ImageFit::Stretch),
    ]
}

/// A number setting's decimals: the ones its values are sent with, or two
/// with no step.
fn decimals(setting: &RendererSetting) -> usize {
    setting.decimals().map_or(2, |places| places as usize)
}

/// A control for each declared setting this rox knows, showing the value
/// the target's session would be sent. `set` stores a picked value by key,
/// already held to the declaration. `scrub` hands out a number's drag state
/// by key.
pub(crate) fn setting_rows<P: 'static>(
    declared: &[RendererSetting],
    stored: &Value,
    scrub: &mut dyn FnMut(&str) -> ScrubState,
    edit: &ValueEdit,
    set: impl Fn(&mut P, &str, Value, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Vec<SettingRow> {
    let (values, _) = renderer_settings::values(declared, stored);
    let mut rows = Vec::new();

    for setting in declared {
        let Some(kind) = setting.kind() else {
            continue;
        };
        let value = values.get(&setting.key).cloned().unwrap_or(Value::Null);
        let key = setting.key.clone();
        let set = set.clone();

        let control = match kind {
            SettingKind::Enum => {
                let current = value.as_str().unwrap_or_default().to_string();
                let choices = setting.choices.clone();

                if choices.len() <= CHIPS_MAX {
                    let options: Vec<(SharedString, usize)> = choices
                        .iter()
                        .enumerate()
                        .map(|(ix, choice)| (SharedString::from(choice.label.clone()), ix))
                        .collect();
                    let at = choices
                        .iter()
                        .position(|choice| choice.value == current)
                        .unwrap_or(0);
                    choices_shared(
                        &options,
                        at,
                        move |this: &mut P, ix, cx| {
                            if let Some(choice) = choices.get(ix) {
                                set(this, &key, Value::from(choice.value.clone()), cx);
                            }
                        },
                        cx,
                    )
                    .into_any_element()
                } else {
                    let options: Vec<(String, SharedString)> = choices
                        .iter()
                        .map(|choice| {
                            (
                                choice.value.clone(),
                                SharedString::from(choice.label.clone()),
                            )
                        })
                        .collect();
                    picker(
                        named("renderer-setting-", &setting.key),
                        current,
                        options,
                        false,
                        move |this: &mut P, picked: String, cx| {
                            set(this, &key, Value::from(picked), cx)
                        },
                        cx,
                    )
                    .into_any_element()
                }
            }

            SettingKind::Bool => toggle(
                value.as_bool().unwrap_or(false),
                move |this: &mut P, on, cx| set(this, &key, Value::from(on), cx),
                cx,
            )
            .into_any_element(),

            SettingKind::Number => {
                let (Some(min), Some(max)) = (setting.min, setting.max) else {
                    continue;
                };
                let span = settings_ui::span(min as f32, max as f32, "")
                    .decimals(decimals(setting))
                    .hard();
                let held = setting.clone();

                settings_ui::scalar(
                    &scrub(&setting.key),
                    edit,
                    value.as_f64().unwrap_or(min) as f32,
                    span,
                    move |this: &mut P, picked, cx| {
                        let picked = Value::from(f64::from(picked).clamp(min, max));
                        if let Some(value) = held.accept(&picked) {
                            set(this, &key, value, cx);
                        }
                    },
                    cx,
                )
                .into_any_element()
            }
        };

        rows.push(SettingRow {
            title: SharedString::from(setting.title.trim().to_string()),
            control,
        });
    }

    rows
}

/// A target's settings as its renderer changes from owner `from` to `to`:
/// what it held goes aside under the old renderer's id, and the new one
/// gets back what it held there before, or nothing. Two renderers that
/// share a key never read each other's value.
pub(crate) fn switch(
    stored: &mut Value,
    aside: &mut BTreeMap<String, Value>,
    from: &str,
    to: &str,
) {
    if from == to {
        return;
    }

    let held = std::mem::take(stored);
    let holds = held.as_object().is_some_and(|held| !held.is_empty());
    if let Some(from) = renderer_id(from)
        && holds
    {
        aside.insert(from.to_string(), held);
    }

    *stored = renderer_id(to)
        .and_then(|to| aside.remove(to))
        .unwrap_or(Value::Null);
}

/// `stored` with `key` set to `value`, every other stored key kept.
pub(crate) fn with(stored: &Value, key: &str, value: Value) -> Value {
    let mut map = stored.as_object().cloned().unwrap_or_default();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_says_how_many_decimals_a_number_shows() {
        let number = |min: f64, step: Option<f64>| {
            let declared = serde_json::json!([{
                "key": "n", "type": "number", "title": "N", "min": min, "max": 100, "step": step
            }]);
            let settings: Vec<RendererSetting> = serde_json::from_value(declared).unwrap();
            decimals(&settings[0])
        };

        assert_eq!(number(0., Some(1.)), 0);
        assert_eq!(number(10., Some(10.)), 0);
        assert_eq!(number(0., Some(0.5)), 1);
        assert_eq!(number(0., Some(0.25)), 2);
        assert_eq!(number(0.05, Some(0.1)), 2);
        assert_eq!(number(0., None), 2);
    }

    #[test]
    fn a_setting_is_stored_beside_the_others() {
        let stored = serde_json::json!({ "look": "ember", "gone": 1 });
        assert_eq!(
            with(&stored, "speed", Value::from(2)),
            serde_json::json!({ "look": "ember", "gone": 1, "speed": 2 })
        );
        assert_eq!(
            with(&Value::Null, "look", Value::from("ice")),
            serde_json::json!({ "look": "ice" })
        );
    }

    #[test]
    fn each_renderer_keeps_its_own_settings_across_a_switch() {
        let mut stored = serde_json::json!({ "look": "ember" });
        let mut aside = BTreeMap::new();

        switch(
            &mut stored,
            &mut aside,
            "plugin:plasma",
            "plugin:plasma-two",
        );
        assert_eq!(stored, Value::Null, "a key the two share isn't handed over");

        stored = serde_json::json!({ "look": "ice" });
        switch(
            &mut stored,
            &mut aside,
            "plugin:plasma-two",
            "plugin:plasma",
        );
        assert_eq!(stored, serde_json::json!({ "look": "ember" }));
        assert_eq!(aside["plasma-two"], serde_json::json!({ "look": "ice" }));
        assert!(!aside.contains_key("plasma"), "taken back out");

        // No renderer picked keeps nothing, and the same one moves nothing.
        switch(&mut stored, &mut aside, "plugin:plasma", "");
        assert_eq!(stored, Value::Null);
        switch(&mut stored, &mut aside, "", "plugin:plasma");
        assert_eq!(stored, serde_json::json!({ "look": "ember" }));
        switch(&mut stored, &mut aside, "plugin:plasma", "plugin:plasma");
        assert_eq!(stored, serde_json::json!({ "look": "ember" }));
    }

    #[test]
    fn a_name_is_made_once() {
        let first = named("renderer-setting-", "look");
        let again = named("renderer-setting-", "look");
        assert!(std::ptr::eq(first, again));
        assert_eq!(first, "renderer-setting-look");
    }
}
