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

use serde::{Deserialize, Deserializer};
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Apply {
    /// A `settings` line, and the renderer goes on.
    #[default]
    Line,
    /// The session starts again with the new value in `hello`.
    Restart,
    /// Anything else, which [`check`] refuses with a reason of its own
    /// rather than leaving serde to word a parser error.
    Unknown,
}

impl<'de> Deserialize<'de> for Apply {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Apply, D::Error> {
        Ok(match Value::deserialize(deserializer)?.as_str() {
            Some("line") => Apply::Line,
            Some("restart") => Apply::Restart,
            _ => Apply::Unknown,
        })
    }
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
            Some(SettingKind::Number) => self
                .min
                .and_then(|min| self.accept(&Value::from(min)))
                .unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    /// Whether a number goes out as an integer: one with a step, whose min,
    /// max and step are all whole, never takes anything else.
    pub fn whole(&self) -> bool {
        let whole = |n: Option<f64>| n.is_some_and(|n| n.fract() == 0.0 && n.abs() < 2f64.powi(53));
        whole(self.min) && whole(self.max) && whole(self.step)
    }

    /// How many decimals a stepped number's values have: as many as its min
    /// or its step is written with, whichever has more. None without a step.
    pub fn decimals(&self) -> Option<i32> {
        let step = self.step?;
        Some(decimals_of(self.min.unwrap_or(0.0)).max(decimals_of(step)))
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

                let (Some(step), Some(places)) = (self.step, self.decimals()) else {
                    return Some(Value::from(number));
                };

                // Rounded to the step's decimals after the snap, so a tenth
                // step never leaves as 0.6000000000000001.
                let snapped = min + ((number - min) / step).round() * step;
                let scale = 10f64.powi(places);
                let snapped = ((snapped * scale).round() / scale).clamp(min, max);

                match self.whole() {
                    true => Some(Value::from(snapped as i64)),
                    false => Some(Value::from(snapped)),
                }
            }
        }
    }
}

/// The fewest decimals that write `n`, up to [`DECIMALS_MAX`].
fn decimals_of(n: f64) -> i32 {
    (0..DECIMALS_MAX)
        .find(|&places| {
            let scaled = n * 10f64.powi(places);
            (scaled - scaled.round()).abs() < 1e-6
        })
        .unwrap_or(DECIMALS_MAX)
}

/// The most decimals a stepped number is rounded to.
const DECIMALS_MAX: i32 = 6;

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

        if setting.apply == Apply::Unknown {
            return refuse("has an apply that isn't line or restart");
        }

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

        // An apply this rox doesn't know is refused in words, as a value of
        // any other type is.
        for apply in [json!("soon"), json!(3), json!(null)] {
            assert_eq!(
                refused(json!([{ "key": "a", "type": "bool", "title": "A", "apply": apply }])),
                "plugin.json: setting 0 has an apply that isn't line or restart"
            );
        }
    }

    #[test]
    fn a_whole_number_goes_out_whole_and_a_step_never_drifts() {
        let settings = parsed(json!([
            { "key": "rate", "type": "number", "title": "Rate", "min": 10, "max": 60, "step": 10 },
            { "key": "mix", "type": "number", "title": "Mix", "min": 0, "max": 1, "step": 0.1 },
            { "key": "gain", "type": "number", "title": "Gain", "min": 0.05, "max": 1, "step": 0.1 },
            { "key": "free", "type": "number", "title": "Free", "min": 0, "max": 100 }
        ]));
        assert_eq!(check(&settings), Ok(()));

        let (sent, _) = values(
            &settings,
            &json!({ "rate": 50.0, "mix": 0.6000000000000001 }),
        );
        // As written on the wire, key by key, since key order follows
        // serde_json's features.
        let written = |key: &str| sent[key].to_string();
        assert_eq!(written("rate"), "50");
        assert_eq!(written("mix"), "0.6");
        assert_eq!(written("gain"), "0.05");
        assert_eq!(written("free"), "0.0");

        // Every value a tenth step can land on reads as it's written.
        for tenth in 0..=10 {
            let picked = settings[1].accept(&json!(f64::from(tenth) * 0.1)).unwrap();
            assert_eq!(picked.as_f64(), Some(f64::from(tenth) / 10.0), "{picked}");
        }
        assert_eq!(settings[2].accept(&json!(0.62)), Some(json!(0.65)));
        assert_eq!(settings[0].accept(&json!(44)), Some(json!(40)));
        assert!(settings[0].whole() && !settings[1].whole() && !settings[3].whole());
    }

    #[test]
    fn a_field_a_setting_doesnt_know_is_ignored() {
        let settings = parsed(json!([
            { "key": "look", "type": "bool", "title": "Look", "hint": "from a later rox", "group": 2 }
        ]));
        assert_eq!(check(&settings), Ok(()));
        assert_eq!(settings[0].default_value(), json!(false));
    }

    /// The stand-in host checks a manifest's settings as rox does, so an
    /// author meets rox's refusal before rox. Its limits are these, and run
    /// against the same declarations it refuses what this refuses, in the
    /// same words, and sends the same values.
    #[test]
    fn the_stand_in_holds_settings_to_these_rules() {
        let source = include_str!("../../../examples/renderers/standin.py");
        let constant = |name: &str| -> i64 {
            let line = source
                .lines()
                .find(|line| line.starts_with(&format!("{name} = ")))
                .unwrap_or_else(|| panic!("standin.py has no {name}"));
            line[name.len() + 3..]
                .split("  #")
                .next()
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or_else(|| panic!("{name} isn't a whole number"))
        };
        assert_eq!(constant("MAX_SETTINGS"), MAX_SETTINGS as i64);
        assert_eq!(constant("MAX_CHOICES"), MAX_CHOICES as i64);
        assert_eq!(constant("KEY_MAX"), KEY_MAX as i64);
        assert_eq!(constant("TEXT_MAX"), TEXT_MAX as i64);
        assert_eq!(constant("VALUE_MAX"), VALUE_MAX as i64);
        assert_eq!(constant("DECIMALS_MAX"), i64::from(DECIMALS_MAX));

        let python = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success());
        if !python {
            eprintln!("skipped the stand-in's run: no python3 on PATH");
            return;
        }

        let bool_named = |title: &str| json!([{ "key": "a", "type": "bool", "title": title }]);
        let many: Vec<Value> = (0..=MAX_SETTINGS)
            .map(|i| json!({ "key": format!("k{i}"), "type": "bool", "title": "K" }))
            .collect();
        let cases = vec![
            json!([
                { "key": "look", "type": "enum", "title": "Look", "default": "ice",
                  "choices": [{ "value": "ember", "label": "Ember" }, { "value": "ice", "label": "Ice" }] },
                { "key": "speed", "type": "number", "title": "Speed", "min": 0.5, "max": 4, "step": 0.5 },
                { "key": "rings", "type": "bool", "title": "Rings", "apply": "restart" },
                { "key": "later", "type": "colour", "title": "From a later rox", "apply": "soon" }
            ]),
            json!([
                { "key": "rate", "type": "number", "title": "Rate", "min": 10, "max": 60, "step": 10 },
                { "key": "mix", "type": "number", "title": "Mix", "min": 0, "max": 1, "step": 0.1 },
                { "key": "gain", "type": "number", "title": "Gain", "min": 0.05, "max": 1, "step": 0.1 },
                { "key": "free", "type": "number", "title": "Free", "min": 0, "max": 100, "hint": "ignored" }
            ]),
            json!([{ "key": "a", "type": "number", "title": "A", "min": 1 }]),
            json!([{ "key": "a", "type": "enum", "title": "A", "choices": [] }]),
            json!([{ "key": "a", "type": "bool", "title": "A", "apply": "soon" }]),
            json!([{ "key": "Look", "type": "bool", "title": "Look" }]),
            json!([{ "key": "a", "type": "bool", "title": "A" }, { "key": "a", "type": "bool", "title": "B" }]),
            bool_named(" "),
            bool_named("A\u{202E}B"),
            bool_named(&"x".repeat(TEXT_MAX + 1)),
            json!([{ "key": "a", "type": "bool", "title": "A", "default": "yes" }]),
            json!([{ "key": "a", "type": "enum", "title": "A", "step": 1,
                     "choices": [{ "value": "x", "label": "X" }] }]),
            json!([{ "key": "a", "type": "bool", "title": "A",
                     "choices": [{ "value": "x", "label": "X" }] }]),
            json!([{ "key": "a", "type": "number", "title": "A", "min": 2, "max": 1 }]),
            json!([{ "key": "a", "type": "number", "title": "A", "min": 0, "max": 1, "step": 0 }]),
            json!([{ "key": "a", "type": "enum", "title": "A",
                     "choices": [{ "value": "v".repeat(VALUE_MAX + 1), "label": "V" }] }]),
            json!([{ "key": "a", "type": "enum", "title": "A",
                     "choices": [{ "value": "x", "label": "X" }, { "value": "x", "label": "Y" }] }]),
            json!([{ "key": "a", "type": "number", "title": "A", "min": 0, "max": 1,
                     "choices": [{ "value": "x", "label": "X" }] }]),
            Value::from(many),
        ];
        let stored = json!({
            "look": "ember", "speed": 1.6, "rings": true, "gone": 1,
            "rate": 44, "mix": 0.30000000000000004, "gain": 0.62, "free": 12.5
        });

        let script = r#"
import json, sys
sys.path.insert(0, sys.argv[1])
import standin
cases, stored = json.load(sys.stdin)
out = []
for case in cases:
    why = standin.check_settings(case)
    out.append([why, None if why else standin.settings_for(case, stored)[0]])
print(json.dumps(out))
"#;
        let examples = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/renderers");
        let mut child = std::process::Command::new("python3")
            .args(["-I", "-c", script, examples])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("python3 runs");
        std::io::Write::write_all(
            child.stdin.as_mut().unwrap(),
            json!([cases, stored]).to_string().as_bytes(),
        )
        .unwrap();
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        let answers: Vec<(Option<String>, Value)> =
            serde_json::from_slice(&out.stdout).expect("the stand-in answers each case");

        for (case, (said, sent)) in cases.iter().zip(answers) {
            let declared = parsed(case.clone());
            assert_eq!(said, check(&declared).err(), "{case}");
            if said.is_none() {
                assert_eq!(sent, Value::Object(values(&declared, &stored).0), "{case}");
            }
        }
    }
}
