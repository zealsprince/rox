//! A renderer's own settings (ADR 37, amended 2026-11-26): what its manifest
//! declares under `capabilities.renderer.settings`, and the values a render
//! target holds for them. The declaration is a stranger's, so its text is
//! held to what the inspector can show plainly, and a value is checked
//! against it before it reaches the renderer: a declared key, one of its
//! choices, inside its range. Nothing here is a grant. A setting tells the
//! renderer only what the user picked for it.
//!
//! Titles and labels are the package's own words, shown as written, the
//! way a plugin's `config_schema` titles are.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::manifest::{FILE, misleads};

/// The most settings one renderer can declare.
pub const MAX_SETTINGS: usize = 32;

/// The most choices one enum setting can offer.
pub const MAX_CHOICES: usize = 32;

/// The longest key, in bytes.
pub const KEY_MAX: usize = 32;

/// The longest title or choice label, in characters.
pub const TEXT_MAX: usize = 48;

/// The longest choice value, in bytes.
pub const VALUE_MAX: usize = 64;

/// One declared setting.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RendererSetting {
    /// The name its value goes under on the wire.
    pub key: String,
    /// `enum`, `bool` or `number`. A type this rox doesn't know is skipped,
    /// so a renderer written for a later rox still loads.
    #[serde(rename = "type")]
    pub kind: String,
    /// What the inspector calls it. Untrusted text.
    pub title: String,
    /// An enum's choices, in the order they're offered.
    #[serde(default)]
    pub choices: Vec<SettingChoice>,
    /// A number's range, both required for a number.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// A number's step from `min`. Left out, any number in the range.
    #[serde(default)]
    pub step: Option<f64>,
    /// Left out: an enum's first choice, false, or a number's `min`.
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub apply: Apply,
}

/// One choice of an enum setting.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SettingChoice {
    /// What the renderer is sent.
    pub value: String,
    /// What the user is shown. Untrusted text.
    pub label: String,
}

/// How a change reaches a running renderer.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Apply {
    /// A `settings` line, and the renderer goes on.
    #[default]
    Line,
    /// The session starts again with the new value in `hello`.
    Restart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Enum,
    Bool,
    Number,
}

impl RendererSetting {
    /// None for a type this rox doesn't know.
    pub fn kind(&self) -> Option<SettingKind> {
        match self.kind.as_str() {
            "enum" => Some(SettingKind::Enum),
            "bool" => Some(SettingKind::Bool),
            "number" => Some(SettingKind::Number),
            _ => None,
        }
    }

    /// The value a target holds until the user picks one.
    pub fn default_value(&self) -> Value {
        if let Some(value) = self.default.as_ref().and_then(|d| self.accept(d)) {
            return value;
        }

        match self.kind() {
            Some(SettingKind::Enum) => self
                .choices
                .first()
                .map_or(Value::Null, |choice| Value::from(choice.value.clone())),
            Some(SettingKind::Bool) => Value::from(false),
            Some(SettingKind::Number) => self.min.map_or(Value::Null, Value::from),
            None => Value::Null,
        }
    }

    /// `value` as the renderer is sent it, or None when it isn't one this
    /// setting takes. A number off its step lands on the nearest one.
    pub fn accept(&self, value: &Value) -> Option<Value> {
        match self.kind()? {
            SettingKind::Enum => {
                let text = value.as_str()?;
                self.choices
                    .iter()
                    .any(|choice| choice.value == text)
                    .then(|| Value::from(text))
            }

            SettingKind::Bool => value.as_bool().map(Value::from),

            SettingKind::Number => {
                let (min, max) = (self.min?, self.max?);
                let number = value.as_f64().filter(|n| n.is_finite())?;
                if !(min..=max).contains(&number) {
                    return None;
                }

                let snapped = match self.step {
                    Some(step) => (min + ((number - min) / step).round() * step).clamp(min, max),
                    None => number,
                };
                Some(Value::from(snapped))
            }
        }
    }
}

/// The settings a session is sent: every declared key this rox knows, with
/// the stored value where it's one the declaration takes, the default
/// where it isn't. The second list names each stored key that was set
/// aside, undeclared or out of range, for the log.
pub fn values(settings: &[RendererSetting], stored: &Value) -> (Map<String, Value>, Vec<String>) {
    let held = stored.as_object();
    let mut out = Map::new();
    let mut dropped = Vec::new();

    for setting in settings.iter().filter(|s| s.kind().is_some()) {
        let picked = held.and_then(|held| held.get(&setting.key));
        let value = match picked.map(|value| setting.accept(value)) {
            Some(Some(value)) => value,
            Some(None) => {
                dropped.push(setting.key.clone());
                setting.default_value()
            }
            None => setting.default_value(),
        };
        out.insert(setting.key.clone(), value);
    }

    if let Some(held) = held {
        let known = |key: &str| settings.iter().any(|s| s.key == key && s.kind().is_some());
        dropped.extend(held.keys().filter(|key| !known(key)).cloned());
    }

    (out, dropped)
}

/// Whether going from `old` to `new` moves a setting the renderer reads
/// only when it starts.
pub fn restarts(
    settings: &[RendererSetting],
    old: &Map<String, Value>,
    new: &Map<String, Value>,
) -> bool {
    settings
        .iter()
        .filter(|s| s.apply == Apply::Restart)
        .any(|s| old.get(&s.key) != new.get(&s.key))
}

/// A declaration rox can show and hold values for. One reason refuses the
/// manifest, on the Renderers page.
pub(crate) fn check(settings: &[RendererSetting]) -> Result<(), String> {
    if settings.len() > MAX_SETTINGS {
        return Err(format!(
            "{FILE}: a renderer declares more than {MAX_SETTINGS} settings"
        ));
    }

    for (at, setting) in settings.iter().enumerate() {
        let refuse = |why: &str| Err(format!("{FILE}: setting {at} {why}"));

        let key_ok = !setting.key.is_empty()
            && setting.key.len() <= KEY_MAX
            && setting.key.starts_with(|c: char| c.is_ascii_lowercase())
            && setting
                .key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !key_ok {
            return refuse(&format!(
                "has a key that isn't a lowercase name of at most {KEY_MAX} letters, digits and underscores"
            ));
        }
        if settings[..at]
            .iter()
            .any(|earlier| earlier.key == setting.key)
        {
            return refuse("has the same key as one before it");
        }
        plain(&setting.title).or_else(|why| refuse(&format!("has a title that {why}")))?;

        let Some(kind) = setting.kind() else {
            continue;
        };

        let ranged = setting.min.is_some() || setting.max.is_some() || setting.step.is_some();
        match kind {
            SettingKind::Enum => {
                if ranged {
                    return refuse("is an enum with a min, max or step");
                }
                if setting.choices.is_empty() || setting.choices.len() > MAX_CHOICES {
                    return refuse(&format!("needs 1 to {MAX_CHOICES} choices"));
                }
                for (ix, choice) in setting.choices.iter().enumerate() {
                    let value = &choice.value;
                    if value.is_empty() || value.len() > VALUE_MAX || value.chars().any(misleads) {
                        return refuse(&format!(
                            "has a choice value that's empty, over {VALUE_MAX} bytes or holds a control character"
                        ));
                    }
                    if setting.choices[..ix].iter().any(|c| c.value == *value) {
                        return refuse("offers the same choice twice");
                    }
                    plain(&choice.label)
                        .or_else(|why| refuse(&format!("has a choice label that {why}")))?;
                }
            }

            SettingKind::Bool => {
                if ranged || !setting.choices.is_empty() {
                    return refuse("is a bool with choices or a range");
                }
            }

            SettingKind::Number => {
                if !setting.choices.is_empty() {
                    return refuse("is a number with choices");
                }
                let (Some(min), Some(max)) = (setting.min, setting.max) else {
                    return refuse("is a number without a min and a max");
                };
                if !(min.is_finite() && max.is_finite() && min < max) {
                    return refuse("has a range that isn't min below max");
                }
                if setting
                    .step
                    .is_some_and(|step| !(step.is_finite() && step > 0.0))
                {
                    return refuse("has a step that isn't above 0");
                }
            }
        }

        if let Some(default) = &setting.default
            && setting.accept(default).is_none()
        {
            return refuse("has a default it doesn't take");
        }
    }

    Ok(())
}

/// Text the inspector shows as a row's name: something, of a size it can
/// show, with no character that could hide or reorder what's around it.
fn plain(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("is empty".into());
    }
    if text.trim().chars().count() > TEXT_MAX {
        return Err(format!("is over {TEXT_MAX} characters"));
    }
    if text.chars().any(misleads) {
        return Err("has a control or direction character".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(value: Value) -> Vec<RendererSetting> {
        serde_json::from_value(value).expect("parses")
    }

    fn look_and_speed() -> Vec<RendererSetting> {
        parsed(json!([
            {
                "key": "look", "type": "enum", "title": "Look",
                "choices": [
                    { "value": "ember", "label": "Ember" },
                    { "value": "ice", "label": "Ice" }
                ],
                "default": "ice"
            },
            { "key": "speed", "type": "number", "title": "Speed", "min": 0.5, "max": 4, "step": 0.5 },
            { "key": "rings", "type": "bool", "title": "Rings", "apply": "restart" },
            { "key": "later", "type": "colour", "title": "From a later rox" }
        ]))
    }

    #[test]
    fn a_declaration_checks_and_skips_a_type_it_doesnt_know() {
        let settings = look_and_speed();
        assert_eq!(check(&settings), Ok(()));
        assert_eq!(settings[2].apply, Apply::Restart);
        assert_eq!(settings[3].kind(), None);

        let (values, dropped) = values(&settings, &Value::Null);
        assert_eq!(
            Value::Object(values),
            json!({ "look": "ice", "speed": 0.5, "rings": false })
        );
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_stored_value_is_held_to_the_declaration() {
        let settings = look_and_speed();
        let stored = json!({
            "look": "lava",
            "speed": 1.6,
            "rings": true,
            "gone": 3,
            "later": "#fff"
        });

        let (values, mut dropped) = values(&settings, &stored);
        assert_eq!(
            Value::Object(values),
            json!({ "look": "ice", "speed": 1.5, "rings": true })
        );
        dropped.sort();
        assert_eq!(dropped, ["gone", "later", "look"]);

        assert_eq!(settings[1].accept(&json!(9)), None, "past the max");
        assert_eq!(settings[1].accept(&json!("2")), None, "a string");
        assert_eq!(settings[0].accept(&json!("ember")), Some(json!("ember")));
    }

    #[test]
    fn only_a_restart_setting_restarts() {
        let settings = look_and_speed();
        let (old, _) = values(&settings, &Value::Null);

        let mut look = old.clone();
        look.insert("look".into(), json!("ember"));
        assert!(!restarts(&settings, &old, &look));

        let mut rings = old.clone();
        rings.insert("rings".into(), json!(true));
        assert!(restarts(&settings, &old, &rings));
    }

    #[test]
    fn a_declaration_rox_cant_show_is_refused() {
        let refused = |value: Value| check(&parsed(value)).unwrap_err();

        assert!(
            refused(json!([{ "key": "Look", "type": "bool", "title": "Look" }])).contains("key")
        );
        assert!(
            refused(json!([
                { "key": "a", "type": "bool", "title": "A" },
                { "key": "a", "type": "bool", "title": "B" }
            ]))
            .contains("same key")
        );
        assert!(refused(json!([{ "key": "a", "type": "bool", "title": " " }])).contains("empty"));
        assert!(
            refused(json!([{ "key": "a", "type": "bool", "title": "A\u{202E}B" }]))
                .contains("direction")
        );
        assert!(
            refused(json!([{ "key": "a", "type": "enum", "title": "A", "choices": [] }]))
                .contains("choices")
        );
        assert!(
            refused(json!([{
                "key": "a", "type": "enum", "title": "A",
                "choices": [{ "value": "x", "label": "X" }, { "value": "x", "label": "Y" }]
            }]))
            .contains("twice")
        );
        assert!(
            refused(json!([{ "key": "a", "type": "number", "title": "A", "min": 1 }]))
                .contains("min and a max")
        );
        assert!(
            refused(json!([{ "key": "a", "type": "number", "title": "A", "min": 2, "max": 1 }]))
                .contains("min below max")
        );
        assert!(
            refused(json!([{ "key": "a", "type": "number", "title": "A", "min": 0, "max": 1, "step": 0 }]))
                .contains("step")
        );
        assert!(
            refused(json!([{ "key": "a", "type": "bool", "title": "A", "default": "yes" }]))
                .contains("default")
        );
        let many: Vec<Value> = (0..=MAX_SETTINGS)
            .map(|i| json!({ "key": format!("k{i}"), "type": "bool", "title": "K" }))
            .collect();
        assert!(refused(Value::from(many)).contains("more than"));

        // An unknown apply is a serde refusal, as any malformed field is.
        assert!(
            serde_json::from_value::<Vec<RendererSetting>>(json!([
                { "key": "a", "type": "bool", "title": "A", "apply": "soon" }
            ]))
            .is_err()
        );
    }
}
