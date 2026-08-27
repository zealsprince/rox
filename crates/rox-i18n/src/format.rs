//! Numbers and dates through ICU4X, targeted at the active locale. One
//! formatter set lives behind a lock and rebuilds on locale switch; the
//! data is compiled in, so a locale we ship can't fail to load and CJK
//! locales later are a data question, not a code one.

use std::sync::{OnceLock, RwLock};

use fixed_decimal::{Decimal, FloatPrecision};
use fluent_bundle::FluentValue;
use icu_calendar::Date;
use icu_datetime::fieldsets::{YMD, YMDT};
use icu_datetime::DateTimeFormatter;
use icu_decimal::DecimalFormatter;
use icu_locale_core::Locale;
use icu_time::{DateTime, Time};
use intl_memoizer::concurrent::IntlLangMemoizer;

struct Formatters {
    number: DecimalFormatter,
    date: DateTimeFormatter<YMD>,
    datetime: DateTimeFormatter<YMDT>,
}

static FORMATTERS: OnceLock<RwLock<Formatters>> = OnceLock::new();

fn build(id: &str) -> Formatters {
    let locale: Locale = id.parse().unwrap_or_else(|_| {
        log::error!("i18n: {id} is not a parseable locale, formatting from root data");
        Locale::UNKNOWN
    });
    Formatters {
        number: DecimalFormatter::try_new((&locale).into(), Default::default())
            .expect("decimal data is compiled in"),
        date: DateTimeFormatter::try_new((&locale).into(), YMD::medium())
            .expect("date data is compiled in"),
        datetime: DateTimeFormatter::try_new((&locale).into(), YMDT::medium())
            .expect("datetime data is compiled in"),
    }
}

fn with<T>(f: impl FnOnce(&Formatters) -> T) -> T {
    let lock = FORMATTERS.get_or_init(|| RwLock::new(build(crate::locale())));
    f(&lock.read().unwrap())
}

/// Rebuild for a fresh locale; the setter in lib.rs calls this before it
/// swaps the chain so a repaint never sees mixed languages and formats.
pub(crate) fn retarget(id: &str) {
    let lock = FORMATTERS.get_or_init(|| RwLock::new(build(id)));
    *lock.write().unwrap() = build(id);
}

/// An integer with the locale's grouping: 12,345 in English, 12.345 in
/// German and Italian, 12 345 in French.
pub fn format_int(n: i64) -> String {
    with(|f| f.number.format(&Decimal::from(n)).to_string())
}

/// A float rounded to at most `max_frac` places, locale decimal mark and
/// grouping applied.
pub fn format_float(value: f64, max_frac: u8) -> String {
    let decimal = Decimal::try_from_f64(value, FloatPrecision::Magnitude(-i16::from(max_frac)))
        .unwrap_or_else(|_| Decimal::from(0));
    with(|f| f.number.format(&decimal).to_string())
}

/// A calendar date in the locale's medium form: Aug 25, 2026 against
/// en-CA, 25.08.2026 against de, and 2026年8月25日 once a zh locale
/// ships, all from the same call.
pub fn format_date(year: i32, month: u8, day: u8) -> String {
    let Ok(date) = Date::try_new_iso(year, month, day) else {
        return format!("{year}-{month:02}-{day:02}");
    };
    with(|f| f.date.format(&date).to_string())
}

/// Date plus wall-clock time, medium date with hour and minute; the
/// locale decides 12 or 24 hour convention.
pub fn format_datetime(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> String {
    let (Ok(date), Ok(time)) = (
        Date::try_new_iso(year, month, day),
        Time::try_new(hour, minute, 0, 0),
    ) else {
        return format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}");
    };
    with(|f| f.datetime.format(&DateTime { date, time }).to_string())
}

/// A measured value with its unit symbol: "44.1 kHz" against en-CA,
/// "44,1 kHz" against de and fr. Only the number is a locale question.
/// The symbols the app shows are SI (Hz, kHz, dB, kbps), and SI spells
/// those the same everywhere, so translating them would invent variants
/// no reader wants.
///
/// The separator is a plain space. Typographically French and German
/// both want a non-breaking one here, and this is the single place that
/// changes if it ever matters enough to chase.
pub fn format_unit(value: f64, max_frac: u8, symbol: &str) -> String {
    format!("{} {symbol}", format_float(value, max_frac))
}

/// A percentage, sign included. Placement is a locale question rather
/// than a notational one - French and German set the sign off with a
/// space where English and Italian close it up - so the whole string
/// comes from the locale and the number rides the usual hook.
///
/// The value arrives already scaled: pass 50.0 for half, not 0.5.
pub fn format_percent(value: f64) -> String {
    crate::t!("unit-percent", value = value).to_string()
}

/// An ISO `YYYY-MM-DD` string in the locale's medium form. Dates stored
/// in files stay ISO because that's a format machines agree on across
/// versions; this is the one-way trip to what a reader sees.
///
/// Anything that isn't an ISO date comes back untouched. The workspace
/// card's dates are author-editable text, so a hand-typed "spring 2019"
/// is a normal thing to find there and mangling it would be worse than
/// passing it through.
pub fn format_iso_date(text: &str) -> String {
    let parts: Vec<&str> = text.trim().split('-').collect();
    let [year, month, day] = parts[..] else {
        return text.to_string();
    };
    let (Ok(year), Ok(month), Ok(day)) =
        (year.parse::<i32>(), month.parse::<u8>(), day.parse::<u8>())
    else {
        return text.to_string();
    };
    format_date(year, month, day)
}

/// The hook every Fluent bundle carries: number placeables render here
/// instead of through Fluent's bare `to_string`, so `{ $count }` gets
/// locale grouping without call sites pre-formatting. Formatting follows
/// the active locale even when the message fell back to English, which
/// is what a German reader wants from an untranslated string.
pub(crate) fn fluent_number(
    value: &FluentValue<'_>,
    _memoizer: &IntlLangMemoizer,
) -> Option<String> {
    let FluentValue::Number(number) = value else {
        return None;
    };
    if let Some(max) = number.options.maximum_fraction_digits {
        return Some(format_float(number.value, max.min(u8::MAX as usize) as u8));
    }
    if number.value.fract() == 0.0 && number.value.abs() < 1e15 {
        Some(format_int(number.value as i64))
    } else {
        Some(format_float(number.value, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_follows_locale() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("de"));
        assert_eq!(format_int(12345), "12.345");
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_int(12345), "12,345");
    }

    /// The decimal mark is the whole point; the symbol rides along
    /// untouched because SI doesn't translate.
    #[test]
    fn units_localize_the_number_and_leave_the_symbol() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("de"));
        assert_eq!(format_unit(44.1, 1, "kHz"), "44,1 kHz");
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_unit(44.1, 1, "kHz"), "44.1 kHz");
    }

    /// French sets the sign off with a space, English closes it up, and
    /// neither is a thing the call site should be deciding.
    #[test]
    fn percent_placement_is_the_locales_call() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("fr"));
        assert_eq!(format_percent(50.0), "50 %");
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_percent(50.0), "50%");
    }

    /// Stored dates are ISO so files stay readable across versions;
    /// only the display end is localized.
    #[test]
    fn iso_dates_render_in_the_locale() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_iso_date("2026-01-02"), format_date(2026, 1, 2));
    }

    /// The workspace card's date fields are author-editable text, so
    /// free-form entries have to survive the trip rather than vanish.
    #[test]
    fn hand_typed_dates_pass_through_untouched() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_iso_date("spring 2019"), "spring 2019");
        assert_eq!(format_iso_date(""), "");
        assert_eq!(format_iso_date("2026-13-45"), "2026-13-45");
    }

    #[test]
    fn dates_follow_locale() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("it"));
        let it = format_date(2026, 8, 25);
        crate::set_locale(Some("en-CA"));
        let en = format_date(2026, 8, 25);
        assert_ne!(it, en);
        assert!(en.contains("2026"));
    }
}
