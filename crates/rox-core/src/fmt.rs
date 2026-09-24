//! The small readouts the whole app shares: durations, counts, and ages.

use gpui::SharedString;

pub fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Blank when zero: the scanner stores a missing tag as 0.
pub fn fmt_num(n: u16) -> SharedString {
    if n == 0 {
        SharedString::default()
    } else {
        n.to_string().into()
    }
}

pub fn fmt_time(secs: f64) -> String {
    fmt_time_padded(secs, 1)
}

/// Minutes zero-padded to `digits`, so a per-frame clock holds one width.
pub fn fmt_time_padded(secs: f64, digits: usize) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!(
        "{m:0digits$}:{:02}",
        (secs - (m * 60) as f64).floor() as u64
    )
}

pub fn fmt_ago(secs: i64) -> String {
    let secs = secs.max(0);
    // One message per unit, not a shared "{value}{unit} ago" frame: German
    // says "vor 2 Wo." and the number doesn't always lead.
    let (value, key) = match secs {
        s if s < 60 => return rox_i18n::t!("ago-just-now").to_string(),
        s if s < 3600 => (s / 60, "ago-minutes"),
        s if s < 86400 => (s / 3600, "ago-hours"),
        s if s < 86400 * 7 => (s / 86400, "ago-days"),
        s if s < 86400 * 365 => (s / (86400 * 7), "ago-weeks"),
        s => (s / (86400 * 365), "ago-years"),
    };
    rox_i18n::t!(key, count = value as u64).to_string()
}

/// Decimal units like the file managers show.
pub fn fmt_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB"] {
        if value < 1000. {
            break;
        }
        value /= 1000.;
        unit = next;
    }
    match unit {
        "B" => rox_i18n::format::format_unit(bytes as f64, 0, "B"),
        "KB" => rox_i18n::format::format_unit(value, 0, "KB"),
        _ => rox_i18n::format::format_unit(value, 1, unit),
    }
}

pub fn fmt_date(unix_secs: i64) -> String {
    use chrono::Datelike;
    let Some(utc) = chrono::DateTime::from_timestamp(unix_secs, 0) else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    rox_i18n::format::format_date(local.year(), local.month() as u8, local.day() as u8)
}

pub fn fmt_datetime(unix_secs: i64) -> String {
    if unix_secs <= 0 {
        return String::new();
    }
    let Some(utc) = chrono::DateTime::from_timestamp(unix_secs, 0) else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    local.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn fmt_total(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// The largest unit that fits and the one under it, "3 weeks, 2 days".
/// Each unit and the joiner are their own messages, for the reason
/// [`fmt_ago`]'s are.
pub fn fmt_span(secs: u64) -> String {
    const UNITS: &[(u64, &str)] = &[
        (86_400 * 365, "span-years"),
        (86_400 * 7, "span-weeks"),
        (86_400, "span-days"),
        (3_600, "span-hours"),
        (60, "span-minutes"),
        (1, "span-seconds"),
    ];
    let Some(top) = UNITS.iter().position(|(span, _)| secs >= *span) else {
        return rox_i18n::t!("span-seconds", count = 0u64).to_string();
    };
    let (span, key) = UNITS[top];
    let first = rox_i18n::t!(key, count = secs / span).to_string();
    let Some(&(next_span, next_key)) = UNITS.get(top + 1) else {
        return first;
    };
    let rest = (secs % span) / next_span;
    if rest == 0 {
        return first;
    }
    let second = rox_i18n::t!(next_key, count = rest).to_string();
    rox_i18n::t!("span-pair", first = first, second = second).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(key: &str, n: u64) -> String {
        rox_i18n::t!(key, count = n).to_string()
    }

    fn pair(first: String, second: String) -> String {
        rox_i18n::t!("span-pair", first = first, second = second).to_string()
    }

    /// Asserted as composition rather than English text, so the suite passes
    /// on a machine whose OS locale isn't English.
    #[test]
    fn spans_read_in_two_units() {
        // Both sides resolve the locale separately, so hold the lock.
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        assert_eq!(fmt_span(0), unit("span-seconds", 0));
        assert_eq!(fmt_span(45), unit("span-seconds", 45));
        assert_eq!(
            fmt_span(90),
            pair(unit("span-minutes", 1), unit("span-seconds", 30))
        );
        assert_eq!(
            fmt_span(3_600 * 5 + 60 * 12),
            pair(unit("span-hours", 5), unit("span-minutes", 12))
        );
        assert_eq!(fmt_span(86_400 * 7 * 3), unit("span-weeks", 3));
        assert_eq!(
            fmt_span(86_400 * 23),
            pair(unit("span-weeks", 3), unit("span-days", 2))
        );
        assert_eq!(
            fmt_span(86_400 * 365 + 86_400 * 14),
            pair(unit("span-years", 1), unit("span-weeks", 2))
        );
    }

    #[test]
    fn spans_read_like_english_in_english() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(fmt_span(0), "0 seconds");
        assert_eq!(fmt_span(90), "1 minute, 30 seconds");
        assert_eq!(fmt_span(86_400 * 23), "3 weeks, 2 days");
        rox_i18n::set_locale(Some("de"));
        assert_eq!(fmt_span(90), "1 Minute, 30 Sekunden");
        assert_eq!(fmt_span(86_400 * 23), "3 Wochen, 2 Tage");
        rox_i18n::set_locale(None);
    }

    #[test]
    fn ages_read_in_one_unit() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        assert_eq!(fmt_ago(-5), rox_i18n::t!("ago-just-now"));
        assert_eq!(fmt_ago(59), rox_i18n::t!("ago-just-now"));
        assert_eq!(fmt_ago(60), rox_i18n::t!("ago-minutes", count = 1u64));
        assert_eq!(fmt_ago(3600), rox_i18n::t!("ago-hours", count = 1u64));
        assert_eq!(fmt_ago(86400 * 3), rox_i18n::t!("ago-days", count = 3u64));
        assert_eq!(fmt_ago(86400 * 14), rox_i18n::t!("ago-weeks", count = 2u64));
        assert_eq!(
            fmt_ago(86400 * 365 * 2),
            rox_i18n::t!("ago-years", count = 2u64)
        );
    }
}
