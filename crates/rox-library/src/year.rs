//! The date a tag claims, for the library's year and date columns. The tag's
//! own string is never rewritten: the tag editor shows it as written, and only
//! these derived numbers reach the projection.
//!
//! Taggers write the date however the user's locale reads it, so this takes
//! any form with a four-digit year in it (`2007-06-12`, `12.06.2007`,
//! `06/12/2007`) instead of lofty's ISO-only timestamp parse. The year is what
//! sorts and filters; month and day only feed the display.

use lofty::tag::{ItemKey, Tag};

/// A parsed date. Month and day are 0 when the tag doesn't carry them, and
/// a day never comes without a month.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Date {
    /// `YYYY-MM-DD`, or `YYYY-MM` when the day is unknown. None when the tag
    /// stops at the year, which the year field already carries.
    pub fn iso(self) -> Option<String> {
        match (self.year, self.month, self.day) {
            (0, _, _) | (_, 0, _) => None,
            (y, m, 0) => Some(format!("{y:04}-{m:02}")),
            (y, m, d) => Some(format!("{y:04}-{m:02}-{d:02}")),
        }
    }
}

/// The date off one parsed tag, all zero where it carries none. A recording
/// date with no year in it falls back to the year field.
pub fn read(tag: &Tag) -> Date {
    [ItemKey::RecordingDate, ItemKey::Year]
        .into_iter()
        .filter_map(|key| tag.get_string(key))
        .map(parse_date)
        .find(|date| date.year != 0)
        .unwrap_or_default()
}

/// The plausible year in a date string, 0 when there isn't one.
pub fn parse(value: &str) -> u16 {
    parse_date(value).year
}

/// The date in a tag string. With more than one candidate year, a string
/// that leads with a year is ISO-ordered and keeps the first; anything else
/// (day-first, month-first) keeps the last. The month and day come from the
/// groups beside the year, read in that order's direction.
///
/// A date with the year last and both other groups at 12 or under can't be
/// told apart by its numbers. Dots and dashes read day-first, the European
/// habit that writes them; slashes read month-first.
pub fn parse_date(value: &str) -> Date {
    let value = value.trim();
    let groups: Vec<(usize, &str)> = value
        .char_indices()
        .filter(|(_, c)| c.is_ascii_digit())
        .fold(Vec::new(), |mut groups: Vec<(usize, &str)>, (ix, _)| {
            match groups.last_mut() {
                Some((start, run)) if *start + run.len() == ix => {
                    *run = &value[*start..=ix];
                }
                _ => groups.push((ix, &value[ix..=ix])),
            }
            groups
        });

    let plausible = |run: &str| {
        run.parse::<u16>()
            .ok()
            .filter(|year| (1000..=2999).contains(year))
    };
    // An eight-digit run is a compact `20070612`.
    if let Some(&(_, run)) = groups.iter().find(|(_, run)| run.len() == 8)
        && let Some(year) = plausible(&run[..4])
    {
        return with_parts(year, Some(&run[4..6]), Some(&run[6..8]));
    }

    let candidates: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, (_, run))| run.len() == 4 && plausible(run).is_some())
        .map(|(ix, _)| ix)
        .collect();
    let Some(&first) = candidates.first() else {
        return Date::default();
    };

    let at = if groups[first].0 == 0 {
        first
    } else {
        *candidates.last().unwrap_or(&first)
    };
    let year = plausible(groups[at].1).unwrap_or(0);
    let run = |ix: usize| groups.get(ix).map(|&(_, run)| run);

    if at == 0 {
        return with_parts(year, run(1), run(2));
    }

    // Year last: the two groups before it are the day and month in some order.
    let before: Vec<(usize, &str)> = groups[..at].iter().rev().take(2).copied().collect();
    match before[..] {
        [(_, b), (a_start, a)] => {
            let sep = value[a_start + a.len()..].chars().next();
            let day_first = match (small(a), small(b)) {
                (Some(a), _) if a > 12 => true,
                (_, Some(b)) if b > 12 => false,
                _ => sep != Some('/'),
            };
            if day_first {
                with_parts(year, Some(b), Some(a))
            } else {
                with_parts(year, Some(a), Some(b))
            }
        }
        [(_, a)] => with_parts(year, Some(a), None),
        _ => with_parts(year, None, None),
    }
}

/// A one- or two-digit group as a number.
fn small(run: &str) -> Option<u8> {
    (run.len() <= 2).then(|| run.parse().ok()).flatten()
}

/// Out-of-range parts drop to 0, and a day can't stand without a month.
fn with_parts(year: u16, month: Option<&str>, day: Option<&str>) -> Date {
    let month = month
        .and_then(small)
        .filter(|m| (1..=12).contains(m))
        .unwrap_or(0);
    let day = if month == 0 {
        0
    } else {
        day.and_then(small)
            .filter(|d| (1..=31).contains(d))
            .unwrap_or(0)
    };
    Date { year, month, day }
}

#[cfg(test)]
mod tests {
    use super::{Date, parse, parse_date};

    fn date(year: u16, month: u8, day: u8) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn every_common_date_form_yields_its_year() {
        for value in [
            "2007",
            "2007-06-12",
            "2007-06",
            "12.06.2007",
            "12/06/2007",
            "06/12/2007",
            "2007.06.12",
            "2007-06-12T00:00:00",
            "20070612",
            "06/2007",
            " 2007 ",
        ] {
            assert_eq!(parse(value), 2007, "{value:?}");
        }
    }

    #[test]
    fn every_common_date_form_yields_its_parts() {
        for (value, want) in [
            ("2007", date(2007, 0, 0)),
            ("2007-06-12", date(2007, 6, 12)),
            ("2007-06", date(2007, 6, 0)),
            ("2007.06.12", date(2007, 6, 12)),
            ("2007/06/12", date(2007, 6, 12)),
            ("2007-06-12T00:00:00", date(2007, 6, 12)),
            ("20070612", date(2007, 6, 12)),
            ("12.06.2007", date(2007, 6, 12)),
            ("12-06-2007", date(2007, 6, 12)),
            ("06/2007", date(2007, 6, 0)),
            ("06.2007", date(2007, 6, 0)),
            (" 2007 ", date(2007, 0, 0)),
        ] {
            assert_eq!(parse_date(value), want, "{value:?}");
        }
    }

    #[test]
    fn slashes_read_month_first_unless_the_numbers_say_otherwise() {
        assert_eq!(parse_date("06/12/2007"), date(2007, 6, 12));
        assert_eq!(parse_date("25/12/2007"), date(2007, 12, 25));
        assert_eq!(parse_date("12/25/2007"), date(2007, 12, 25));
        assert_eq!(parse_date("25.12.2007"), date(2007, 12, 25));
        assert_eq!(parse_date("12.25.2007"), date(2007, 12, 25));
    }

    #[test]
    fn out_of_range_parts_keep_only_the_year() {
        assert_eq!(parse_date("2007-13-01"), date(2007, 0, 0));
        assert_eq!(parse_date("2007-06-32"), date(2007, 6, 0));
        assert_eq!(parse_date("32.13.2007"), date(2007, 0, 0));
        assert_eq!(parse_date("2007-00-00"), date(2007, 0, 0));
    }

    #[test]
    fn iso_form_stops_where_the_tag_does() {
        assert_eq!(date(2007, 6, 12).iso().as_deref(), Some("2007-06-12"));
        assert_eq!(date(2007, 6, 0).iso().as_deref(), Some("2007-06"));
        assert_eq!(date(2007, 0, 0).iso(), None);
        assert_eq!(Date::default().iso(), None);
    }

    #[test]
    fn junk_yields_no_year() {
        for value in [
            "",
            "unknown",
            "no year here",
            "0000",
            "12.06.07",
            "123456",
            "3000",
        ] {
            assert_eq!(parse(value), 0, "{value:?}");
            assert_eq!(parse_date(value), Date::default(), "{value:?}");
        }
    }

    #[test]
    fn several_candidates_pick_by_order() {
        assert_eq!(parse("2007/2008"), 2007, "ISO order keeps the first");
        assert_eq!(parse("1999 remaster, 12.06.2007"), 1999);
        assert_eq!(
            parse("rec. 1999, 12.06.2007"),
            2007,
            "day-first keeps the last"
        );
        assert_eq!(parse_date("rec. 1999, 12.06.2007"), date(2007, 6, 12));
    }
}
