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
use std::collections::HashMap;

use gpui::{AnyElement, Context, SharedString, prelude::*};
use rox_canvas::ImageFit;
use rox_panel_api::panel::{ScrubState, ValueEdit};
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{choices_shared, picker, toggle};
use rox_plugins::renderer_settings::{self, RendererSetting, SettingKind};
use serde_json::Value;

use crate::canvas::target::SCALES;

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

/// A number setting's decimals: as many as its step has, or two with no
/// step.
fn decimals(step: Option<f64>) -> usize {
    let Some(step) = step else {
        return 2;
    };

    (0..4)
        .find(|&places| {
            let scaled = step * 10f64.powi(places);
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(3) as usize
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
                    .decimals(decimals(setting.step))
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
        assert_eq!(decimals(Some(1.)), 0);
        assert_eq!(decimals(Some(10.)), 0);
        assert_eq!(decimals(Some(0.5)), 1);
        assert_eq!(decimals(Some(0.25)), 2);
        assert_eq!(decimals(None), 2);
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
    fn a_name_is_made_once() {
        let first = named("renderer-setting-", "look");
        let again = named("renderer-setting-", "look");
        assert!(std::ptr::eq(first, again));
        assert_eq!(first, "renderer-setting-look");
    }
}
