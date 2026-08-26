# ADR 27: i18n: Fluent messages, ICU4X formatting, one locale static

**Status:** Decided

Decision: interface strings live in Fluent (.ftl) files, one per locale under
crates/rox-i18n/locales, compiled into the binary and resolved at render time
through a `t!` macro that answers in SharedString. en-CA is the source locale:
every key exists there first, the app's existing spelling ("favourites") is
already Canadian, and every resolution chain ends there so a hole in a
translation shows English rather than a bare key. The active locale is a
process-global behind one setter, the theme system's exact shape: settings
carry `language: Option<String>` (None follows the OS via sys-locale plus
langneg negotiation), `set_language` swaps the static and refreshes every
window, and startup seeds it beside `set_theme`. Numbers and dates never go
through Fluent's own stringification: ICU4X (compiled data, so a locale is
data not code) renders them, both through explicit helpers
(`format::format_int`, `format_date`, ...) and through a formatter hook every
bundle carries, so a `{ $count }` placeable gets locale grouping while plural
selection still sees the raw value. Bidi isolation marks are off until an RTL
locale forces the question. Shipped locales are en-CA, de, fr, it; a locale is
one row in the `LOCALES` registry plus one ftl file, and a parity test fails
the build when any locale's key inventory drifts from the source. APIs that
demand `&'static str` (the settings row DSL, `panel::choices`) bridge through
`t_static`, a per-locale-and-key memoized leak; each use marks an API that
wants widening to SharedString, and the widened twin (`choices_shared`) is
where new translated call sites land.

Alternatives: rust-i18n or gettext instead of Fluent; fluent-templates'
static_loader instead of hand-held bundles; formatting numbers inside Fluent;
icu's chrono adapter for the date helpers; threading a locale parameter
through render calls instead of the static; per-crate locale files instead of
one registry; shipping translations as runtime-loaded files.

Trade: Fluent costs more ceremony than rust-i18n's flat key-value for the
same four Latin locales we ship today, and buys CLDR plural rules and
selectors, which is precisely what makes zh and ja (bare "other" plurals,
different date orders) translation work rather than engineering work later.
Hand-held bundles over fluent-templates is a page of code for control over
the memoizer, the isolation flag, and the formatter hook, all three of which
we set. The ICU hook means formatting follows the UI locale even when a
message fell back to English, which reads as intended rather than as a bug.
Compiled-in locales and ICU data grow the binary by a few megabytes but keep
translations atomic with the code that keys into them; runtime loading would
buy community locale drops at the cost of version skew between keys and
binaries, the wrong trade while keys churn. The static outside gpui's
reactivity means repaints are explicit and strings cached in entity state
catch up on their next notify; live switching is a settings-window act, so
that lag is invisible in practice. t_static's leak is bounded by keys times
locales visited, kilobytes against the alternative of widening every
`&'static str` signature in one sweep; the widening still happens, page by
page as extraction reaches it. What this decision does not cover, recorded
as open: RTL (isolation marks and gpui's bidi story), locale-aware library
collation (icu_collator, a different sort for the same shelf), and CJK font
fallback plus IME, which ride gpui's text system and want checking against a
real zh or ja locale when one lands.
