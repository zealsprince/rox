//! The control socket's settings verbs: `settings.list`, `settings.get` and
//! `settings.set` (ADR 22). Only the keys in [`KEYS`] are reachable, each by
//! a stable snake_case name, and a set writes the file and then runs what the
//! setting's own switch on the Settings window or the menu runs, so the app
//! follows at once.
//!
//! Deliberately absent: account credentials and the broadcast password,
//! anything that moves where data lives or what programs rox runs (library
//! folders have their own verbs), the audio device and exclusive mode, the
//! update channel and staged downloads, and every approval gate: the plugin
//! and AI switches, a plugin's enable, a shader's approval.

use gpui::App;
use serde_json::{Value, json};

use rox_core::settings::{self, ChromeSide, ChromeStyle, RatingStyle, Settings, Theme};
use rox_design::palette;
use rox_ipc::RpcError;

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Bool(bool),
    Choice(&'static str),
    Number(f64),
}

impl Val {
    fn json(&self) -> Value {
        match self {
            Val::Bool(on) => json!(on),
            Val::Choice(name) => json!(name),
            Val::Number(n) => json!(n),
        }
    }

    fn on(&self) -> bool {
        matches!(self, Val::Bool(true))
    }

    fn choice(&self) -> &'static str {
        match self {
            Val::Choice(name) => name,
            _ => "",
        }
    }

    fn number(&self) -> f64 {
        match self {
            Val::Number(n) => *n,
            _ => 0.0,
        }
    }
}

enum Kind {
    Bool,
    /// The saved names, as the settings file spells them.
    Choice(fn() -> Vec<&'static str>),
    Number {
        min: f64,
        max: f64,
    },
}

impl Kind {
    fn parse(&self, value: &Value) -> Result<Val, String> {
        match self {
            Kind::Bool => value
                .as_bool()
                .map(Val::Bool)
                .ok_or_else(|| "takes true or false".to_string()),

            Kind::Choice(names) => {
                let names = names();
                value
                    .as_str()
                    .and_then(|given| names.iter().find(|name| **name == given))
                    .map(|name| Val::Choice(name))
                    .ok_or_else(|| format!("takes one of {}", names.join(", ")))
            }

            Kind::Number { min, max } => value
                .as_f64()
                .filter(|n| n.is_finite() && (*min..=*max).contains(n))
                .map(Val::Number)
                .ok_or_else(|| format!("takes a number from {min} to {max}")),
        }
    }

    fn describe(&self) -> Value {
        match self {
            Kind::Bool => json!({ "type": "bool" }),
            Kind::Choice(names) => json!({ "type": "choice", "values": names() }),
            Kind::Number { min, max } => json!({ "type": "number", "min": min, "max": max }),
        }
    }
}

struct Key {
    name: &'static str,
    kind: Kind,
    /// The value as saved.
    read: fn(&Settings) -> Val,
    /// The file write, given a value [`Kind::parse`] accepted.
    write: fn(&mut Settings, &Val),
    /// The live half: what the setting's switch runs after its write.
    apply: fn(&Val, &mut App),
}

fn languages() -> Vec<&'static str> {
    std::iter::once("system")
        .chain(rox_i18n::LOCALES.iter().map(|locale| locale.id))
        .collect()
}

fn themes() -> Vec<&'static str> {
    vec!["dark", "light", "system"]
}

fn theme_of(name: &str) -> Theme {
    match name {
        "dark" => Theme::Dark,
        "light" => Theme::Light,
        _ => Theme::System,
    }
}

fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => "system",
    }
}

/// A saved id that isn't shipped reads as what it negotiates to: the
/// language the UI is really in.
fn language_name(saved: Option<&str>) -> &'static str {
    match saved {
        None => "system",
        Some(id) => languages()
            .into_iter()
            .find(|known| *known == id)
            .unwrap_or_else(rox_i18n::locale),
    }
}

/// The menubar's show and hide rows run these after their write.
fn rebuild_menu(cx: &mut App) {
    crate::workspace::native_menu::rebuild(cx);
}

static KEYS: &[Key] = &[
    Key {
        name: "language",
        kind: Kind::Choice(languages),
        read: |s| Val::Choice(language_name(s.language.as_deref())),
        write: |s, v| {
            s.language = match v.choice() {
                "system" => None,
                id => Some(id.to_string()),
            }
        },
        apply: |v, cx| {
            let language = match v.choice() {
                "system" => None,
                id => Some(id),
            };
            settings::set_language(language, cx);
        },
    },
    Key {
        name: "theme",
        kind: Kind::Choice(themes),
        read: |s| Val::Choice(theme_name(s.theme)),
        write: |s, v| s.theme = theme_of(v.choice()),
        apply: |v, cx| settings::set_theme(theme_of(v.choice()), cx),
    },
    Key {
        name: "design_mode",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.design_mode),
        write: |s, v| s.design_mode = v.on(),
        apply: |v, cx| {
            settings::set_design_mode(v.on(), cx);
            rebuild_menu(cx);
        },
    },
    Key {
        name: "resize_lock",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.resize_lock),
        write: |s, v| s.resize_lock = v.on(),
        apply: |v, cx| settings::set_resize_lock(v.on(), cx),
    },
    Key {
        name: "seams",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.seams),
        write: |s, v| s.look.bundle.appearance.seams = v.on(),
        apply: |v, cx| settings::set_seams(v.on(), cx),
    },
    Key {
        name: "show_readings",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.show_readings),
        write: |s, v| s.show_readings = v.on(),
        apply: |v, cx| settings::set_show_readings(v.on(), cx),
    },
    Key {
        name: "hide_menubar",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.hide_menubar),
        write: |s, v| s.look.bundle.appearance.hide_menubar = v.on(),
        apply: |v, cx| {
            settings::set_hide_menubar(v.on(), cx);
            rebuild_menu(cx);
        },
    },
    Key {
        name: "menubar_tasks",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.menubar_buttons.tasks),
        write: |s, v| s.look.bundle.appearance.menubar_buttons.tasks = v.on(),
        apply: |v, cx| {
            let mut buttons = settings::menubar_buttons();
            buttons.tasks = v.on();
            settings::set_menubar_buttons(buttons, cx);
        },
    },
    Key {
        name: "menubar_sleep",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.menubar_buttons.sleep),
        write: |s, v| s.look.bundle.appearance.menubar_buttons.sleep = v.on(),
        apply: |v, cx| {
            let mut buttons = settings::menubar_buttons();
            buttons.sleep = v.on();
            settings::set_menubar_buttons(buttons, cx);
        },
    },
    Key {
        name: "menubar_rescan",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.menubar_buttons.rescan),
        write: |s, v| s.look.bundle.appearance.menubar_buttons.rescan = v.on(),
        apply: |v, cx| {
            let mut buttons = settings::menubar_buttons();
            buttons.rescan = v.on();
            settings::set_menubar_buttons(buttons, cx);
        },
    },
    Key {
        name: "os_decorations",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.os_decorations),
        write: |s, v| s.look.bundle.appearance.os_decorations = v.on(),
        apply: |v, cx| {
            settings::set_os_decorations(v.on());
            crate::workspace::apply_decorations(cx);
            rebuild_menu(cx);
        },
    },
    Key {
        name: "chrome_style",
        kind: Kind::Choice(|| vec!["icons", "traffic"]),
        read: |s| {
            Val::Choice(match s.look.bundle.appearance.chrome_style {
                ChromeStyle::Icons => "icons",
                ChromeStyle::Traffic => "traffic",
            })
        },
        write: |s, v| {
            s.look.bundle.appearance.chrome_style = match v.choice() {
                "traffic" => ChromeStyle::Traffic,
                _ => ChromeStyle::Icons,
            }
        },
        apply: |v, cx| {
            settings::set_chrome_style(match v.choice() {
                "traffic" => ChromeStyle::Traffic,
                _ => ChromeStyle::Icons,
            });
            crate::workspace::refresh_all_windows(cx);
        },
    },
    Key {
        name: "chrome_side",
        kind: Kind::Choice(|| vec!["left", "right"]),
        read: |s| {
            Val::Choice(match s.look.bundle.appearance.chrome_side {
                ChromeSide::Left => "left",
                ChromeSide::Right => "right",
            })
        },
        write: |s, v| {
            s.look.bundle.appearance.chrome_side = match v.choice() {
                "left" => ChromeSide::Left,
                _ => ChromeSide::Right,
            }
        },
        apply: |v, cx| {
            settings::set_chrome_side(match v.choice() {
                "left" => ChromeSide::Left,
                _ => ChromeSide::Right,
            });
            crate::workspace::refresh_all_windows(cx);
        },
    },
    Key {
        name: "art_theming",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.art_theming),
        write: |s, v| s.look.bundle.appearance.art_theming = v.on(),
        apply: |v, cx| {
            palette::set_art_theming(v.on(), cx);
            rebuild_menu(cx);
        },
    },
    Key {
        name: "rating_style",
        kind: Kind::Choice(|| vec!["stars", "numeric"]),
        read: |s| {
            Val::Choice(match s.look.bundle.appearance.rating_style {
                RatingStyle::Stars => "stars",
                RatingStyle::Numeric => "numeric",
            })
        },
        write: |s, v| {
            s.look.bundle.appearance.rating_style = match v.choice() {
                "numeric" => RatingStyle::Numeric,
                _ => RatingStyle::Stars,
            }
        },
        apply: |v, cx| {
            let style = match v.choice() {
                "numeric" => RatingStyle::Numeric,
                _ => RatingStyle::Stars,
            };
            settings::set_rating_style(style, cx);
        },
    },
    Key {
        name: "rating_dots",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.look.bundle.appearance.rating_dots),
        write: |s, v| s.look.bundle.appearance.rating_dots = v.on(),
        apply: |v, cx| settings::set_rating_dots(v.on(), cx),
    },
    Key {
        name: "app_font_size",
        kind: Kind::Number {
            min: palette::FONT_SIZE_MIN as f64,
            max: palette::FONT_SIZE_MAX as f64,
        },
        read: |s| Val::Number(s.app_font_size as f64),
        write: |s, v| s.app_font_size = v.number() as f32,
        apply: |v, cx| {
            palette::set_app_font_size(v.number() as f32, cx);
            // The palette repaints through each window's handle, which misses
            // a window mid-update, as the zoom chords note.
            cx.refresh_windows();
        },
    },
    Key {
        name: "quit_to_tray",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.quit_to_tray),
        write: |s, v| s.quit_to_tray = v.on(),
        apply: |v, cx| {
            settings::set_quit_to_tray(v.on());
            crate::integrations::tray::sync(cx);
            rebuild_menu(cx);
            cx.refresh_windows();
        },
    },
    Key {
        name: "check_updates",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.check_updates),
        write: |s, v| s.check_updates = v.on(),
        // Read at the next launch's check; nothing on screen follows it.
        apply: |_, _| {},
    },
    Key {
        name: "experimental",
        kind: Kind::Bool,
        read: |s| Val::Bool(s.experimental),
        write: |s, v| s.experimental = v.on(),
        apply: |v, cx| {
            settings::set_experimental(v.on(), cx);
            rebuild_menu(cx);
        },
    },
];

fn key_named(name: &str) -> Result<&'static Key, RpcError> {
    KEYS.iter().find(|key| key.name == name).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "no setting {name:?} can be read or set here; settings.list names the ones that can"
        ))
    })
}

fn key_param(params: &Value) -> Result<&'static Key, RpcError> {
    let name = params
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("takes {\"key\": a name from settings.list}"))?;

    key_named(name)
}

pub fn list() -> Value {
    let saved = Settings::load();
    let keys: Vec<Value> = KEYS
        .iter()
        .map(|key| {
            let mut row = key.kind.describe();
            row["key"] = json!(key.name);
            row["value"] = (key.read)(&saved).json();
            row
        })
        .collect();

    json!({ "settings": keys })
}

pub fn get(params: &Value) -> Result<Value, RpcError> {
    let key = key_param(params)?;

    Ok(json!({ "key": key.name, "value": (key.read)(&Settings::load()).json() }))
}

/// The write lands first, the way the settings switches order it, so a
/// live setter that rereads the file sees the new value.
pub fn set(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let key = key_param(params)?;
    let given = params
        .get("value")
        .ok_or_else(|| RpcError::invalid_params("takes {\"key\", \"value\"}"))?;
    let value = key
        .kind
        .parse(given)
        .map_err(|why| RpcError::invalid_params(format!("{} {why}", key.name)))?;

    let stored = value.clone();
    Settings::update(move |s| (key.write)(s, &stored));
    (key.apply)(&value, cx);

    Ok(json!({ "key": key.name, "value": value.json() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(kind: &Kind) -> Vec<Val> {
        match kind {
            Kind::Bool => vec![Val::Bool(true), Val::Bool(false)],
            Kind::Choice(names) => names().into_iter().map(Val::Choice).collect(),
            Kind::Number { min, max } => vec![
                Val::Number(*min),
                Val::Number(*max),
                Val::Number((min + max) / 2.0),
            ],
        }
    }

    #[test]
    fn every_key_reads_back_what_was_set() {
        let mut store = Settings::default();

        for key in KEYS {
            for value in samples(&key.kind) {
                let parsed = key.kind.parse(&value.json()).expect(key.name);
                (key.write)(&mut store, &parsed);
                assert_eq!((key.read)(&store), value, "{}", key.name);
            }
        }
    }

    #[test]
    fn an_unknown_key_is_refused() {
        for name in ["nope", "broadcast.password", "plugins_enabled", ""] {
            let err = key_named(name).err().expect(name);
            assert_eq!(err.code, -32602);
        }
        assert!(key_param(&json!({})).is_err());
        assert!(key_param(&json!({ "key": 3 })).is_err());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused() {
        let kind = |name| &key_named(name).unwrap().kind;

        assert!(kind("design_mode").parse(&json!("true")).is_err());
        assert!(kind("design_mode").parse(&json!(1)).is_err());
        assert!(kind("theme").parse(&json!("purple")).is_err());
        assert!(kind("theme").parse(&json!(true)).is_err());
        assert!(kind("language").parse(&json!("xx-XX")).is_err());
        assert!(kind("app_font_size").parse(&json!("16")).is_err());
        assert!(kind("app_font_size").parse(&json!(4)).is_err());
        assert!(kind("app_font_size").parse(&json!(400)).is_err());

        assert_eq!(kind("language").parse(&json!("de")), Ok(Val::Choice("de")));
        assert_eq!(
            kind("app_font_size").parse(&json!(14)),
            Ok(Val::Number(14.0))
        );
    }

    #[test]
    fn no_key_names_a_secret_or_a_gate() {
        for key in KEYS {
            for word in [
                "password",
                "token",
                "secret",
                "session",
                "account",
                "api",
                "key",
                "approv",
                "plugin",
                "shader",
                "mcp",
                "ai_",
                "device",
                "exclusive",
                "output",
                "folder",
                "root",
                "dir",
                "path",
                "download",
            ] {
                assert!(!key.name.contains(word), "{} names {word}", key.name);
            }
        }
    }

    #[test]
    fn names_are_unique_and_snake_case() {
        let mut seen = std::collections::HashSet::new();

        for key in KEYS {
            assert!(seen.insert(key.name), "{} twice", key.name);
            assert!(
                key.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                key.name
            );
        }
    }
}
