# ADR 27: i18n: Fluent messages, ICU4X formatting, one locale static

**Status:** Decided

Decision: interface strings live in Fluent (`.ftl`) files, one per locale under
`crates/rox-i18n/locales`, compiled into the binary and resolved at render time through a
`t!` macro that returns a `SharedString`.

**en-CA is the source locale.** Every key exists there before it exists anywhere else,
which also makes the parity test possible. The app's existing spelling was
already Canadian ("favourites"), so nothing had to be respelled to adopt it. And every
resolution chain terminates there, so a key a translation hasn't covered yet falls
through to English rather than showing the reader a raw key name.

**The active locale is a process-global behind one setter**, which is the same shape the
theme system already uses. Settings hold `language: Option<String>`, where `None` means
follow the OS, resolved through sys-locale and then langneg negotiation to pick the
closest shipped locale. `set_language` swaps the static and refreshes every open window,
and startup seeds it right beside `set_theme`.

**Numbers and dates never go through Fluent's own stringification.** ICU4X renders them,
using compiled data, so adding a locale is adding data rather than adding code. It's
reached two ways: explicit helpers like `format::format_int` and `format_date` for
call sites that format directly, and a formatter hook installed on every bundle so a
`{ $count }` placeable inside a message gets locale-correct grouping without the message
author doing anything. Ordering matters in the hook: plural selection still sees
the raw numeric value, since it has to choose a plural form before anything is
stringified.

Bidi isolation marks are off until an RTL locale forces the question.

**Shipped locales** are en-CA, de, fr, it, es, pt-BR, ru, uk, ja, and zh-Hans. Adding
one is a row in the `LOCALES` registry plus one ftl file, and a parity test fails the
build if any locale's key inventory drifts from the source, so a translation can't
quietly fall behind the code.

**One bridge for APIs that demand `&'static str`.** The settings row DSL and
`panel::choices` both want static strings, which a runtime-resolved translation isn't, so
they go through `t_static`, a memoized leak keyed by locale and key. Every use of it
marks an API that should be widened to `SharedString`, and the widened twin
(`choices_shared`) is where new translated call sites go.

Alternatives: rust-i18n or gettext instead of Fluent; fluent-templates' `static_loader`
instead of hand-held bundles; formatting numbers inside Fluent; icu's chrono adapter for
the date helpers; threading a locale parameter through render calls instead of using the
static; per-crate locale files instead of one registry; shipping translations as
runtime-loaded files.

Trade: Fluent costs more ceremony than rust-i18n's flat key-value would, and for a
Latin-only set that ceremony would buy nothing. What it buys is CLDR plural rules and
selectors, and those decide whether a language like zh or ja is translation work or
engineering work. Both have a single bare "other" plural form and a different date order
from English. A flat key-value scheme would need per-language special cases in the code.
Fluent puts them in the message file, where translators can reach them.

Hand-writing the bundle loading instead of taking fluent-templates costs about a page of
code and buys control over three things we actually set: the memoizer, the isolation
flag, and the formatter hook.

The ICU hook has a useful consequence beyond correctness. Formatting follows the UI
locale even when a particular message has fallen back to English, so a partially
translated screen shows English words with local number and date formatting, which reads
as a translation in progress rather than as a bug.

Compiling the locales and ICU data into the binary adds a few megabytes, and it keeps
translations atomic with the code that keys into them: a build can't have keys the
translations don't. Runtime loading would allow community locale drops without a
release, at the cost of version skew between a key set and a binary, which is the wrong
trade while the keys are still churning.

The static sits outside gpui's reactivity, so repaints after a language change are
explicit, and any string already cached in entity state catches up on that entity's next
notify rather than immediately. Switching language is something someone does in the
settings window, so that lag isn't observable in practice.

`t_static`'s leak is bounded by the number of keys times the number of locales visited in
one run, which is kilobytes. The alternative was widening every `&'static str` signature
in the app in a single sweep. The widening still happens, page by page, as extraction
reaches each one.

What this decision doesn't cover, recorded as open: RTL, meaning both isolation marks and
whatever gpui's bidi story turns out to be; locale-aware library collation through
`icu_collator`, which is a different sort order for the same shelf and a separate
question from message translation; and CJK font fallback plus IME support, which depend
on gpui's text system and want checking now that zh-Hans and ja actually ship.
