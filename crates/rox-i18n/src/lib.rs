//! App-wide localization: Fluent messages resolved against the active
//! locale, ICU4X behind number and date rendering. The shape mirrors the
//! theme system - one process-global the setter swaps, every read going
//! through an accessor - because strings change for the same reason
//! palettes do: a settings row flips and every window repaints.
//!
//! en-CA is the source locale; its file carries every key, and the
//! resolution chain always ends there so a hole in a translation shows
//! English rather than a bare key. Adding a locale is one row in
//! [`LOCALES`] plus one ftl file; the parity test keeps the files honest.

pub mod format;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock, RwLock};

pub use fluent_bundle::FluentArgs;
use fluent_bundle::FluentResource;
use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use gpui::SharedString;
use unic_langid::LanguageIdentifier;

/// The concurrent bundle: translate is called from any thread that
/// renders or logs, and the default memoizer is single-thread only.
type Bundle = fluent_bundle::concurrent::FluentBundle<FluentResource>;

/// A shipped locale: what the language picker shows and which ftl file
/// backs it. The struct stays unconstructable outside the crate so the
/// registry below is the one list everything derives from.
pub struct LocaleInfo {
    pub id: &'static str,
    pub flag: &'static str,
    pub native: &'static str,
    /// What the picker's search matches besides the native name, all
    /// lowercase: the language and its country as every shipped locale
    /// says them, adjective forms, plain-ASCII spellings of the
    /// accented ones, and the id. The whole point of the picker's
    /// search is someone stranded in a language that isn't theirs
    /// typing in their own, so each locale added here earns a row in
    /// everyone else's aliases too.
    pub aliases: &'static [&'static str],
    ftl: &'static str,
}

/// The locale every key exists in and the end of every fallback chain.
pub const SOURCE_LOCALE: &str = "en-CA";

/// Registry order is picker order. Native names stay in their own
/// language on purpose: a German speaker hunting for theirs scans for
/// "Deutsch", not for whatever the current locale calls it.
pub const LOCALES: &[LocaleInfo] = &[
    LocaleInfo {
        id: "en-CA",
        flag: "🇨🇦",
        native: "English",
        aliases: &[
            "english", "englisch", "anglais", "inglese", "canada", "canadian", "kanada", "en",
            "en-ca",
        ],
        ftl: include_str!("../locales/en-CA/rox.ftl"),
    },
    LocaleInfo {
        id: "de",
        flag: "🇩🇪",
        native: "Deutsch",
        aliases: &[
            "deutsch",
            "german",
            "germany",
            "deutschland",
            "allemand",
            "allemagne",
            "tedesco",
            "germania",
            "de",
        ],
        ftl: include_str!("../locales/de/rox.ftl"),
    },
    LocaleInfo {
        id: "fr",
        flag: "🇫🇷",
        native: "Français",
        aliases: &[
            "français",
            "francais",
            "french",
            "france",
            "französisch",
            "franzosisch",
            "frankreich",
            "francese",
            "francia",
            "fr",
        ],
        ftl: include_str!("../locales/fr/rox.ftl"),
    },
    LocaleInfo {
        id: "it",
        flag: "🇮🇹",
        native: "Italiano",
        aliases: &[
            "italiano",
            "italian",
            "italy",
            "italia",
            "italienisch",
            "italien",
            "italie",
            "it",
        ],
        ftl: include_str!("../locales/it/rox.ftl"),
    },
];

/// Bundles parse once and live for the process; locale switches only
/// change which ones the chain visits.
static BUNDLES: OnceLock<Vec<(LanguageIdentifier, Bundle)>> = OnceLock::new();

/// The resolution chain as indices into [`BUNDLES`], most specific
/// first, source last. Lazily seeded from the OS locale so translate
/// works before init runs (tests, early logging).
static ACTIVE: OnceLock<RwLock<Vec<usize>>> = OnceLock::new();

fn bundles() -> &'static [(LanguageIdentifier, Bundle)] {
    BUNDLES.get_or_init(|| {
        LOCALES
            .iter()
            .map(|loc| {
                let lang: LanguageIdentifier =
                    loc.id.parse().expect("registry ids are static and valid");
                let resource = match FluentResource::try_new(loc.ftl.to_string()) {
                    Ok(resource) => resource,
                    Err((resource, errors)) => {
                        for error in errors {
                            log::error!("i18n: parsing {}: {error}", loc.id);
                        }
                        resource
                    }
                };
                let mut bundle = Bundle::new_concurrent(vec![lang.clone()]);
                // Fluent wraps placeables in FSI/PDI bidi isolate marks by
                // default. None of the shipped locales are bidi and the
                // marks surface as tofu in width measuring, so they stay
                // off until an RTL locale forces the question.
                bundle.set_use_isolating(false);
                // Numbers in placeables render through ICU with the active
                // locale's grouping and decimal mark; plural selection
                // still sees the raw value.
                bundle.set_formatter(Some(format::fluent_number));
                if let Err(errors) = bundle.add_resource(resource) {
                    for error in errors {
                        log::error!("i18n: loading {}: {error}", loc.id);
                    }
                }
                (lang, bundle)
            })
            .collect()
    })
}

fn active() -> &'static RwLock<Vec<usize>> {
    ACTIVE.get_or_init(|| RwLock::new(negotiate(None)))
}

/// Resolve a preference to a chain of shipped locales. None asks the OS;
/// either way the source locale caps the chain so lookups always land.
fn negotiate(pref: Option<&str>) -> Vec<usize> {
    let requested: Vec<LanguageIdentifier> = match pref {
        Some(id) => id.parse().ok().into_iter().collect(),
        None => sys_locale::get_locales()
            .filter_map(|id| id.parse().ok())
            .collect(),
    };
    let available: Vec<LanguageIdentifier> =
        bundles().iter().map(|(lang, _)| lang.clone()).collect();
    let matched = negotiate_languages(&requested, &available, None, NegotiationStrategy::Filtering);
    let mut chain: Vec<usize> = matched
        .iter()
        .filter_map(|lang| available.iter().position(|l| l == *lang))
        .collect();
    let source = LOCALES
        .iter()
        .position(|loc| loc.id == SOURCE_LOCALE)
        .expect("source locale is registered");
    if !chain.contains(&source) {
        chain.push(source);
    }
    chain
}

/// Swap the active locale and retarget the ICU formatters. None follows
/// the OS. Repainting is the caller's, same as the palette setter: the
/// statics sit outside gpui's reactivity on purpose.
pub fn set_locale(pref: Option<&str>) {
    let chain = negotiate(pref);
    let primary = LOCALES[chain[0]].id;
    format::retarget(primary);
    *active().write().unwrap() = chain;
}

/// The locale rendering right now, resolved: asking while set to System
/// answers what System negotiated to.
pub fn locale() -> &'static str {
    LOCALES[active().read().unwrap()[0]].id
}

/// Resolve a message, walking the chain until a locale has it. A `.`
/// reaches into an attribute: `"settings-theme.description"` is the
/// description attribute of `settings-theme`, mirroring ftl syntax.
pub fn translate(key: &str, args: Option<&FluentArgs>) -> SharedString {
    let (id, attr) = match key.split_once('.') {
        Some((id, attr)) => (id, Some(attr)),
        None => (key, None),
    };
    let bundles = bundles();
    let chain = active().read().unwrap().clone();
    for index in chain {
        let (_, bundle) = &bundles[index];
        let Some(message) = bundle.get_message(id) else {
            continue;
        };
        let pattern = match attr {
            Some(attr) => message.get_attribute(attr).map(|a| a.value()),
            None => message.value(),
        };
        let Some(pattern) = pattern else { continue };
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, args, &mut errors);
        for error in errors {
            log::warn!("i18n: formatting {key}: {error}");
        }
        return decorate(text.into_owned()).into();
    }
    missing(key);
    format!("⟦{key}⟧").into()
}

/// Message lookup. `t!("key")` for plain strings, `t!("key", count = n)`
/// for placeables; args land as Fluent variables under their own names.
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::translate($key, None)
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut args = $crate::FluentArgs::new();
        $(args.set(stringify!($name), $value);)+
        $crate::translate($key, Some(&args))
    }};
}

/// A translation for the APIs that demand `&'static str` - the settings
/// row DSL, `panel::choices`, panel names. Resolves once per locale and
/// key, leaks that, and answers from the map after; the leak is bounded
/// by keys times locales visited, which is kilobytes. Every call site
/// is also a marker: the API behind it wants widening to SharedString,
/// and when that lands the call site moves to [`t!`].
pub fn t_static(key: &str) -> &'static str {
    static INTERNED: Mutex<BTreeMap<(usize, String), &'static str>> = Mutex::new(BTreeMap::new());
    let primary = active().read().unwrap()[0];
    let mut interned = INTERNED.lock().unwrap();
    if let Some(text) = interned.get(&(primary, key.to_string())) {
        return text;
    }
    let text: &'static str = Box::leak(translate(key, None).to_string().into_boxed_str());
    interned.insert((primary, key.to_string()), text);
    text
}

/// The pseudo-locale pass, on when ROX_PSEUDOLOCALE is set: every
/// resolved string gains brackets and a third of padding, so a
/// hardcoded literal is the one thing on screen without brackets and a
/// layout that can't absorb German-length text shows it before German
/// does.
fn decorate(text: String) -> String {
    // Not under test: the suite asserts resolved content, and a pseudo
    // var inherited from the dev shell shouldn't repaint the assertions.
    if cfg!(test) {
        return text;
    }
    static PSEUDO: OnceLock<bool> = OnceLock::new();
    if !*PSEUDO.get_or_init(|| std::env::var_os("ROX_PSEUDOLOCALE").is_some()) {
        return text;
    }
    let pad = "~".repeat(text.chars().count().div_ceil(3));
    format!("⟦{text}{pad}⟧")
}

/// A key no locale carries, logged once: repeating it every frame would
/// drown the log from inside a render loop.
fn missing(key: &str) {
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN.lock().unwrap().insert(key.to_string()) {
        log::warn!("i18n: no locale carries {key}");
    }
}

/// Tests flip the global locale, so every test that does takes this
/// lock; without it cargo's parallel runner interleaves locales.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped locale carries exactly the keys and attributes the
    /// source does: a hole falls back silently at runtime, so the test
    /// is where holes surface.
    #[test]
    fn locales_carry_every_source_key() {
        fn inventory(ftl: &str) -> BTreeSet<String> {
            let resource = FluentResource::try_new(ftl.to_string())
                .unwrap_or_else(|_| panic!("locale file parses"));
            let mut keys = BTreeSet::new();
            for entry in resource.entries() {
                if let fluent_syntax::ast::Entry::Message(message) = entry {
                    keys.insert(message.id.name.to_string());
                    for attribute in &message.attributes {
                        keys.insert(format!("{}.{}", message.id.name, attribute.id.name));
                    }
                }
            }
            keys
        }
        let source = LOCALES
            .iter()
            .find(|loc| loc.id == SOURCE_LOCALE)
            .expect("source locale registered");
        let want = inventory(source.ftl);
        for loc in LOCALES {
            let got = inventory(loc.ftl);
            let missing: Vec<_> = want.difference(&got).collect();
            let extra: Vec<_> = got.difference(&want).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{}: missing {missing:?}, extra {extra:?}",
                loc.id
            );
        }
    }

    /// The picker matches aliases without folding case, so the curation
    /// contract is that they arrive lowercase; an uppercase alias would
    /// silently never match.
    #[test]
    fn aliases_are_lowercase_and_present() {
        for loc in LOCALES {
            assert!(!loc.aliases.is_empty(), "{}: no aliases", loc.id);
            for alias in loc.aliases {
                assert_eq!(
                    *alias,
                    alias.to_lowercase(),
                    "{}: alias {alias} isn't lowercase",
                    loc.id
                );
            }
        }
    }

    #[test]
    fn falls_back_to_source_for_unknown_locale() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("pt-BR"));
        assert_eq!(locale(), SOURCE_LOCALE);
    }

    #[test]
    fn attribute_lookup_reaches_into_messages() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("en-CA"));
        let label = translate("settings-language", None);
        let description = translate("settings-language.description", None);
        assert_ne!(label, description);
        assert!(!label.contains('⟦'), "label resolved: {label}");
    }

    #[test]
    fn plurals_select_per_locale() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("de"));
        let one = t!("bake-detail-writes", count = 1);
        let many = t!("bake-detail-writes", count = 2);
        assert_ne!(one, many);
    }
}
