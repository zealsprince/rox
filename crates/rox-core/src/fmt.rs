//! The small readouts the whole app shares: durations, counts, and ages.
//! Nothing here draws anything, so the panels, the settings windows, and the
//! modals all read the same clock without any one of them owning it.

use gpui::SharedString;

/// A track's stored duration as minutes and seconds.
pub fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A track number or year cell: blank when zero, since the scanner stores
/// a missing tag as 0 and a bare 0 reads as data.
pub fn fmt_num(n: u16) -> SharedString {
    if n == 0 {
        SharedString::default()
    } else {
        n.to_string().into()
    }
}

/// The playback clock format the panels share: minutes and seconds.
pub fn fmt_time(secs: f64) -> String {
    fmt_time_padded(secs, 1)
}

/// `fmt_time` with the minutes zero-padded to `digits`, for clocks that
/// tick every frame and need to hold one width for a whole track.
pub fn fmt_time_padded(secs: f64, digits: usize) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!(
        "{m:0digits$}:{:02}",
        (secs - (m * 60) as f64).floor() as u64
    )
}

/// A listen's age as a short readout: seconds up through years, one
/// unit, no calendar math. The stats panel's recents read it too.
pub fn fmt_ago(secs: i64) -> String {
    let secs = secs.max(0);
    // The unit suffix is part of the sentence, not notation: German wants
    // "vor 2 Wo." where English wants "2w ago", and the number does not
    // always lead. So each unit is its own message with the value in it,
    // rather than a shared "{value}{unit} ago" frame.
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

/// A long running time in words: the largest unit that fits and the one
/// under it, "3 weeks, 2 days". The clock readouts stop meaning much past
/// a day, so the library totals carry this beside them.
///
/// Each unit is its own message rather than a shared "{count} {noun}"
/// frame, for the reason [`fmt_ago`] is: a noun that only ever gains an
/// "s" is an English assumption, and German, French, and Italian all
/// inflect differently. The joiner is a message too, since a locale that
/// wants "3 Wochen und 2 Tage" should be able to say so.
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
    // A spent second place drops rather than reading "3 weeks, 0 days".
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

    /// Two units at most, adjacent ones, and a spent second place drops
    /// rather than reading "3 weeks, 0 days".
    ///
    /// Asserted as composition rather than against English text: the
    /// wording belongs to the locale files, and pinning it here would
    /// make the suite fail on a machine whose OS locale isn't English.
    /// What's actually under test is which units get picked.
    #[test]
    fn spans_read_in_two_units() {
        // Held even though nothing here sets a locale: the assertions
        // call fmt_span and the expected side separately, so a sibling
        // test switching locales between the two would split the pair.
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

    /// The wording itself, pinned in one locale under the shared lock so
    /// the plural selectors and the joiner are exercised end to end and
    /// not just against themselves.
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

    /// One unit, the largest that fits, and anything under a minute reads
    /// as now rather than as a number of seconds.
    #[test]
    fn ages_read_in_one_unit() {
        // Same reason as the spans above: both sides resolve separately.
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
