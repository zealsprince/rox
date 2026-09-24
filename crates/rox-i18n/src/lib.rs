//! App-wide localization: Fluent messages resolved against the active
//! locale, ICU4X for numbers and dates. Shaped like the theme system: one
//! process-global the setter swaps, read through accessors.
//!
//! en-CA is the source locale with every key, and every chain ends there so a
//! translation hole shows English, not a bare key. Adding a locale is one row
//! in [`LOCALES`] plus one ftl file; the parity test keeps them honest.

pub mod format;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock, RwLock};

pub use fluent_bundle::FluentArgs;
use fluent_bundle::FluentResource;
use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use gpui::SharedString;
use unic_langid::LanguageIdentifier;

/// Concurrent because translate runs on any thread that renders or logs.
type Bundle = fluent_bundle::concurrent::FluentBundle<FluentResource>;

/// Unconstructable outside the crate, so the registry is the one list
/// everything derives from.
pub struct LocaleInfo {
    pub id: &'static str,
    pub flag: &'static str,
    pub native: &'static str,
    /// Lowercase search terms besides the native name: the language and country
    /// as every shipped locale says them, ASCII spellings, and the id. Someone
    /// stranded in the wrong language types in their own, so each new locale
    /// adds a row to everyone else's aliases.
    pub aliases: &'static [&'static str],
    ftl: &'static str,
}

pub const SOURCE_LOCALE: &str = "en-CA";

/// Registry order is picker order. Native names stay in their own language:
/// a German speaker scans for "Deutsch".
pub const LOCALES: &[LocaleInfo] = &[
    LocaleInfo {
        id: "en-CA",
        flag: "🇨🇦",
        native: "English",
        aliases: &[
            "english",
            "englisch",
            "anglais",
            "inglese",
            "inglés",
            "ingles",
            "inglês",
            "английский",
            "англійська",
            "英語",
            "えいご",
            "eigo",
            "英语",
            "英文",
            "yingyu",
            "yingwen",
            "canada",
            "canadian",
            "kanada",
            "canadá",
            "канада",
            "канадский",
            "канадський",
            "カナダ",
            "加拿大",
            "jianada",
            "en",
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
            "alemán",
            "aleman",
            "alemania",
            "alemão",
            "alemao",
            "alemanha",
            "немецкий",
            "германия",
            "німецька",
            "німеччина",
            "ドイツ語",
            "どいつご",
            "doitsugo",
            "ドイツ",
            "doitsu",
            "德语",
            "德文",
            "deyu",
            "dewen",
            "德国",
            "deguo",
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
            "francés",
            "frances",
            "francês",
            "frança",
            "franca",
            "французский",
            "франция",
            "французька",
            "франція",
            "フランス語",
            "ふらんすご",
            "furansugo",
            "フランス",
            "furansu",
            "法语",
            "法文",
            "fayu",
            "fawen",
            "法国",
            "faguo",
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
            "itália",
            "итальянский",
            "италия",
            "італійська",
            "італія",
            "イタリア語",
            "いたりあご",
            "itariago",
            "イタリア",
            "itaria",
            "意大利语",
            "yidaliyu",
            "意大利",
            "yidali",
            "it",
        ],
        ftl: include_str!("../locales/it/rox.ftl"),
    },
    LocaleInfo {
        id: "es",
        flag: "🇪🇸",
        native: "Español",
        aliases: &[
            "español",
            "espanol",
            "castellano",
            "spanish",
            "spain",
            "españa",
            "espana",
            "spanisch",
            "spanien",
            "espagnol",
            "espagne",
            "spagnolo",
            "spagna",
            "іспанська",
            "іспанія",
            "es",
        ],
        ftl: include_str!("../locales/es/rox.ftl"),
    },
    LocaleInfo {
        id: "pt-BR",
        flag: "🇧🇷",
        native: "Português",
        aliases: &[
            "português",
            "portugues",
            "portuguese",
            "portugiesisch",
            "portugais",
            "portoghese",
            "portugal",
            "brasil",
            "brasileiro",
            "brazil",
            "brazilian",
            "brasilianisch",
            "brasilien",
            "brésilien",
            "bresilien",
            "brésil",
            "bresil",
            "brasiliano",
            "brasile",
            "португальська",
            "бразилія",
            "pt",
            "pt-br",
        ],
        ftl: include_str!("../locales/pt-BR/rox.ftl"),
    },
    LocaleInfo {
        id: "ru",
        flag: "🇷🇺",
        native: "Русский",
        aliases: &[
            "русский",
            "русский язык",
            "russkiy",
            "russkij",
            "російська",
            "росія",
            "russian",
            "russia",
            "россия",
            "rossiya",
            "russisch",
            "russland",
            "russe",
            "russie",
            "russo",
            "ru",
        ],
        ftl: include_str!("../locales/ru/rox.ftl"),
    },
    LocaleInfo {
        id: "uk",
        flag: "🇺🇦",
        native: "Українська",
        aliases: &[
            "українська",
            "українська мова",
            "ukrainska",
            "ukrayinska",
            "ukrainian",
            "ukraine",
            "україна",
            "ukrayina",
            "ukrainisch",
            "ukrainien",
            "ucraino",
            "ucraina",
            "ucraniano",
            "ucrania",
            "ucrânia",
            "украинский",
            "украина",
            "ウクライナ語",
            "うくらいなご",
            "ukurainago",
            "ウクライナ",
            "ukuraina",
            "乌克兰语",
            "wukelanyu",
            "乌克兰",
            "wukelan",
            "uk",
            "ua",
        ],
        ftl: include_str!("../locales/uk/rox.ftl"),
    },
    LocaleInfo {
        id: "ja",
        flag: "🇯🇵",
        native: "日本語",
        aliases: &[
            "日本語",
            "にほんご",
            "nihongo",
            "japanese",
            "japan",
            "японська",
            "японія",
            "japanisch",
            "japonais",
            "japon",
            "giapponese",
            "giappone",
            "ja",
            "jp",
        ],
        ftl: include_str!("../locales/ja/rox.ftl"),
    },
    LocaleInfo {
        id: "zh-Hans",
        flag: "🇨🇳",
        native: "简体中文",
        aliases: &[
            "中文",
            "简体中文",
            "汉语",
            "zhongwen",
            "jiantizhongwen",
            "hanyu",
            "chinese",
            "китайська",
            "китай",
            "simplified chinese",
            "mandarin",
            "chinesisch",
            "chinois",
            "cinese",
            "中国",
            "zhongguo",
            "china",
            "cina",
            "chine",
            "zh",
            "zh-hans",
            "zh-cn",
        ],
        ftl: include_str!("../locales/zh-Hans/rox.ftl"),
    },
];

static BUNDLES: OnceLock<Vec<(LanguageIdentifier, Bundle)>> = OnceLock::new();

/// Indices into [`BUNDLES`], source last. Seeded lazily from the OS locale so
/// translate works before init (tests, early logging).
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
                // Bidi isolate marks render as tofu in width measuring and no
                // shipped locale is RTL.
                bundle.set_use_isolating(false);
                // Plural selection still sees the raw value.
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

/// None asks the OS. The source locale always caps the chain.
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

/// None follows the OS. Repainting is the caller's: the statics are outside
/// gpui's reactivity.
pub fn set_locale(pref: Option<&str>) {
    let chain = negotiate(pref);
    let primary = LOCALES[chain[0]].id;
    format::retarget(primary);
    *active().write().unwrap() = chain;
}

/// Resolved: while set to System, what System negotiated to.
pub fn locale() -> &'static str {
    LOCALES[active().read().unwrap()[0]].id
}

/// Walks the chain until a locale has the key. A `.` reaches into an
/// attribute: `"settings-theme.description"`.
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

/// None when no locale has the key, for optional messages where the missing
/// marker [`translate`] shows would be wrong.
pub fn try_translate(key: &str) -> Option<SharedString> {
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
        let text = bundle.format_pattern(pattern, None, &mut errors);
        for error in errors {
            log::warn!("i18n: formatting {key}: {error}");
        }
        return Some(decorate(text.into_owned()).into());
    }
    None
}

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

/// For APIs that demand `&'static str`. Leaks once per locale and key, which
/// is kilobytes. Each call site marks an API still to widen to SharedString,
/// after which it moves to [`t!`].
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

/// With ROX_PSEUDOLOCALE set, every string gains brackets and a third of
/// padding, exposing hardcoded literals and layouts that can't take German.
fn decorate(text: String) -> String {
    // Never under test: a var inherited from the dev shell would break assertions.
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

/// Logged once, since a render loop would repeat it every frame.
fn missing(key: &str) {
    static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    if SEEN.lock().unwrap().insert(key.to_string()) {
        log::warn!("i18n: no locale defines {key}");
    }
}

/// Every test that flips the global locale takes this lock. Public because
/// other crates (rox-core's spans and paces) format through the same statics.
#[doc(hidden)]
pub static LOCALE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) use LOCALE_TEST_LOCK as TEST_LOCK;

#[cfg(test)]
mod tests {
    use super::*;

    /// A hole falls back silently at runtime, so the test is where it surfaces.
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

    /// Every translation names the same variables the source does. A dropped
    /// or misspelled variable renders as ordinary text, in a language the call
    /// site's author likely can't read. Compared as a set across the whole
    /// message, since the source doesn't use every variable in every branch.
    #[test]
    fn translations_name_the_same_variables_as_the_source() {
        use fluent_syntax::ast;

        fn in_pattern(pattern: &ast::Pattern<&str>, out: &mut BTreeSet<String>) {
            for element in &pattern.elements {
                if let ast::PatternElement::Placeable { expression } = element {
                    in_expression(expression, out);
                }
            }
        }

        fn in_expression(expression: &ast::Expression<&str>, out: &mut BTreeSet<String>) {
            match expression {
                ast::Expression::Inline(inline) => in_inline(inline, out),
                ast::Expression::Select { selector, variants } => {
                    in_inline(selector, out);
                    for variant in variants {
                        in_pattern(&variant.value, out);
                    }
                }
            }
        }

        fn in_inline(inline: &ast::InlineExpression<&str>, out: &mut BTreeSet<String>) {
            match inline {
                ast::InlineExpression::VariableReference { id } => {
                    out.insert(id.name.to_string());
                }
                ast::InlineExpression::Placeable { expression } => in_expression(expression, out),
                ast::InlineExpression::FunctionReference { arguments, .. } => {
                    for positional in &arguments.positional {
                        in_inline(positional, out);
                    }
                    for named in &arguments.named {
                        in_inline(&named.value, out);
                    }
                }
                // A message reference inherits the outer call's arguments.
                _ => {}
            }
        }

        fn inventory(ftl: &str) -> BTreeMap<String, BTreeSet<String>> {
            let resource = FluentResource::try_new(ftl.to_string()).expect("locale file parses");
            let mut out = BTreeMap::new();
            for entry in resource.entries() {
                let ast::Entry::Message(message) = entry else {
                    continue;
                };
                if let Some(value) = &message.value {
                    let mut vars = BTreeSet::new();
                    in_pattern(value, &mut vars);
                    out.insert(message.id.name.to_string(), vars);
                }
                for attribute in &message.attributes {
                    let mut vars = BTreeSet::new();
                    in_pattern(&attribute.value, &mut vars);
                    out.insert(format!("{}.{}", message.id.name, attribute.id.name), vars);
                }
            }
            out
        }

        let source = LOCALES
            .iter()
            .find(|loc| loc.id == SOURCE_LOCALE)
            .expect("source locale registered");
        let want = inventory(source.ftl);
        for loc in LOCALES {
            if loc.id == SOURCE_LOCALE {
                continue;
            }
            for (key, got) in inventory(loc.ftl) {
                // Unknown keys are the parity test's complaint.
                let Some(want) = want.get(&key) else { continue };
                let dropped: Vec<_> = want.difference(&got).collect();
                let invented: Vec<_> = got.difference(want).collect();
                assert!(
                    dropped.is_empty() && invented.is_empty(),
                    "{}: {key} drops {dropped:?} and invents {invented:?}; \
                     the source names {want:?}",
                    loc.id
                );
            }
        }
    }

    /// Every plural selector declares each category its language uses, per
    /// CLDR: a Russian message with English's two branches passes parity but
    /// renders "2 секунда". Non-counting selectors are skipped.
    #[test]
    fn plural_selectors_cover_their_locales_categories() {
        use fluent_syntax::ast;
        use intl_pluralrules::{PluralRuleType, PluralRules};

        const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

        /// Enumerated from the rules, the same question the formatter asks.
        fn needed(lang: &LanguageIdentifier) -> BTreeSet<String> {
            // The rules table is keyed by bare language: en-CA counts like en.
            let bare: LanguageIdentifier = lang
                .language
                .as_str()
                .parse()
                .expect("a language subtag is a valid identifier");
            let rules = PluralRules::create(bare, PluralRuleType::CARDINAL)
                .expect("every shipped locale has cardinal rules");
            (0..=200u64)
                .map(|n| format!("{:?}", rules.select(n).expect("a count selects")).to_lowercase())
                .collect()
        }

        fn declared(pattern: &ast::Pattern<&str>, out: &mut Vec<BTreeSet<String>>) {
            for element in &pattern.elements {
                let ast::PatternElement::Placeable { expression } = element else {
                    continue;
                };
                let ast::Expression::Select { variants, .. } = expression else {
                    continue;
                };
                let mut keys = BTreeSet::new();
                for variant in variants {
                    if let ast::VariantKey::Identifier { name } = &variant.key {
                        keys.insert(name.to_string());
                    }
                    declared(&variant.value, out);
                }
                // Two or more category branches means the message inflects,
                // and must do so completely. One branch is an opt-out.
                if keys.len() >= 2 && keys.iter().all(|k| CATEGORIES.contains(&k.as_str())) {
                    out.push(keys);
                }
            }
        }

        for loc in LOCALES {
            let lang: LanguageIdentifier = loc.id.parse().expect("registry ids are valid");
            let want = needed(&lang);
            let resource =
                FluentResource::try_new(loc.ftl.to_string()).expect("locale file parses");
            for entry in resource.entries() {
                let ast::Entry::Message(message) = entry else {
                    continue;
                };
                let mut selects = Vec::new();
                if let Some(value) = &message.value {
                    declared(value, &mut selects);
                }
                for attribute in &message.attributes {
                    declared(&attribute.value, &mut selects);
                }
                for keys in selects {
                    let missing: Vec<_> = want.difference(&keys).collect();
                    assert!(
                        missing.is_empty(),
                        "{}: {} is missing the {missing:?} plural {}; {} needs {want:?}",
                        loc.id,
                        message.id.name,
                        if missing.len() == 1 {
                            "branch"
                        } else {
                            "branches"
                        },
                        loc.id,
                    );
                }
            }
        }
    }

    /// The picker matches case-sensitively, so aliases must be lowercase.
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
        set_locale(Some("sv-SE"));
        assert_eq!(locale(), SOURCE_LOCALE);
    }

    /// The OS reports zh-CN or pt-BR, not the registry's spelling;
    /// negotiation has to bridge that.
    #[test]
    fn os_spellings_reach_their_locale() {
        let _guard = TEST_LOCK.lock().unwrap();
        for (reported, want) in [
            ("zh-CN", "zh-Hans"),
            ("zh-Hans-CN", "zh-Hans"),
            ("pt-BR", "pt-BR"),
            ("es-MX", "es"),
            ("es-419", "es"),
            ("ru-RU", "ru"),
            ("uk-UA", "uk"),
            ("ja-JP", "ja"),
        ] {
            set_locale(Some(reported));
            assert_eq!(locale(), want, "{reported} should land on {want}");
        }
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

    /// Each locale adds its own synonyms, or translated labels are unsearchable.
    #[test]
    fn keyword_lists_are_per_locale() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("en-CA"));
        let english = try_translate("settings-audio-crossfade.keywords")
            .expect("the source locale defines the list");
        set_locale(Some("de"));
        let german = try_translate("settings-audio-crossfade.keywords")
            .expect("German defines its own list");
        assert_ne!(english, german);
        assert!(german.split_whitespace().any(|term| term == "uebergang"));
    }

    /// No synonyms reads as absent, not as the missing marker.
    #[test]
    fn a_row_without_keywords_answers_none() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("en-CA"));
        assert!(try_translate("settings-audio-crossfade.nonesuch").is_none());
    }

    /// A message reference gets the outer call's arguments; a failed one would
    /// render the literal message name.
    #[test]
    fn a_message_reference_carries_the_callers_args() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_locale(Some("en-CA"));
        let mut args = FluentArgs::new();
        args.set("estimate", "about 2 hours");
        args.set("workers", "4 workers");
        let wrapped = translate("tasks-estimate-at-workers", Some(&args));
        assert_eq!(wrapped, "(about 2 hours at 4 workers)");
    }

    /// A mangled reference still passes parity, so every locale is checked
    /// here.
    #[test]
    fn every_locale_resolves_the_wrapped_estimate() {
        let _guard = TEST_LOCK.lock().unwrap();
        for loc in LOCALES {
            set_locale(Some(loc.id));
            let mut args = FluentArgs::new();
            args.set("estimate", "ESTIMATE");
            args.set("workers", "WORKERS");
            let wrapped = translate("tasks-estimate-at-workers", Some(&args)).to_string();
            assert!(
                wrapped.contains("ESTIMATE") && wrapped.contains("WORKERS"),
                "{}: both arguments should survive, got {wrapped}",
                loc.id
            );
            assert!(
                !wrapped.contains("tasks-estimate-at"),
                "{}: the reference rendered literally, got {wrapped}",
                loc.id
            );
        }
        set_locale(None);
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

/// Case and accent folded, so "prereglages" finds "Préréglages". NFD with
/// combining marks dropped; the sharp s is spelled out first ("ss").
pub fn fold(text: &str) -> String {
    static NFD: OnceLock<icu_normalizer::DecomposingNormalizerBorrowed<'static>> = OnceLock::new();
    let nfd = NFD.get_or_init(icu_normalizer::DecomposingNormalizerBorrowed::new_nfd);
    let lowered = text.to_lowercase().replace('ß', "ss");
    nfd.normalize(&lowered)
        .chars()
        .filter(|c| {
            !icu_properties::CodePointMapData::<icu_properties::props::GeneralCategory>::new()
                .get(*c)
                .eq(&icu_properties::props::GeneralCategory::NonspacingMark)
        })
        .collect()
}

#[cfg(test)]
mod fold_tests {
    use super::fold;

    #[test]
    fn accents_fold_to_their_base_letter() {
        assert_eq!(fold("Préréglages"), "prereglages");
        assert_eq!(fold("Überblenden"), "uberblenden");
        assert_eq!(fold("Città"), "citta");
    }

    #[test]
    fn sharp_s_spells_itself_out() {
        assert_eq!(fold("Größe"), "grosse");
    }

    #[test]
    fn plain_ascii_is_only_lowercased() {
        assert_eq!(fold("Row Height"), "row height");
    }
}
