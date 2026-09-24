//! Numbers and dates through ICU4X for the active locale. One formatter set
//! behind a lock, rebuilt on locale switch; the data is compiled in, so a
//! shipped locale can't fail to load.

use std::sync::{OnceLock, RwLock};

use fixed_decimal::{Decimal, FloatPrecision};
use fluent_bundle::FluentValue;
use icu_calendar::Date;
use icu_datetime::DateTimeFormatter;
use icu_datetime::fieldsets::{YMD, YMDT};
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

/// Called by the setter before it swaps the chain, so a repaint never mixes
/// languages and formats.
pub(crate) fn retarget(id: &str) {
    let lock = FORMATTERS.get_or_init(|| RwLock::new(build(id)));
    *lock.write().unwrap() = build(id);
}

pub fn format_int(n: i64) -> String {
    with(|f| f.number.format(&Decimal::from(n)).to_string())
}

pub fn format_float(value: f64, max_frac: u8) -> String {
    let decimal = Decimal::try_from_f64(value, FloatPrecision::Magnitude(-i16::from(max_frac)))
        .unwrap_or_else(|_| Decimal::from(0));
    with(|f| f.number.format(&decimal).to_string())
}

/// Medium form: Aug 25, 2026 in en-CA, 25.08.2026 in de.
pub fn format_date(year: i32, month: u8, day: u8) -> String {
    let Ok(date) = Date::try_new_iso(year, month, day) else {
        return format!("{year}-{month:02}-{day:02}");
    };
    with(|f| f.date.format(&date).to_string())
}

pub fn format_datetime(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> String {
    let (Ok(date), Ok(time)) = (
        Date::try_new_iso(year, month, day),
        Time::try_new(hour, minute, 0, 0),
    ) else {
        return format!("{year}-{month:02}-{day:02} {hour:02}:{minute:02}");
    };
    with(|f| f.datetime.format(&DateTime { date, time }).to_string())
}

/// Only the number is localized: SI symbols (Hz, kHz, dB, kbps) read the
/// same everywhere. The separator is a plain space, though French and German
/// typography want a non-breaking one; this is the one place to change it.
pub fn format_unit(value: f64, max_frac: u8, symbol: &str) -> String {
    format!("{} {symbol}", format_float(value, max_frac))
}

/// Sign placement is a locale question (French and German set it off with a
/// space), so the whole string comes from the locale. Pass 50.0 for half.
pub fn format_percent(value: f64) -> String {
    crate::t!("unit-percent", value = value).to_string()
}

/// Stored dates stay ISO; this is the one-way trip to display. Anything that
/// isn't an ISO date passes through: workspace card dates are hand-typed text
/// like "spring 2019".
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

/// Installed on every Fluent bundle, so `{ $count }` gets locale grouping
/// without call sites pre-formatting. Follows the active locale even when the
/// message fell back to English.
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

    #[test]
    fn units_localize_the_number_and_leave_the_symbol() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("de"));
        assert_eq!(format_unit(44.1, 1, "kHz"), "44,1 kHz");
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_unit(44.1, 1, "kHz"), "44.1 kHz");
    }

    #[test]
    fn percent_placement_is_the_locales_call() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("fr"));
        assert_eq!(format_percent(50.0), "50 %");
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_percent(50.0), "50%");
    }

    #[test]
    fn iso_dates_render_in_the_locale() {
        let _guard = crate::TEST_LOCK.lock().unwrap();
        crate::set_locale(Some("en-CA"));
        assert_eq!(format_iso_date("2026-01-02"), format_date(2026, 1, 2));
    }

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
