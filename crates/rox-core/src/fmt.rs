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
    let (value, unit) = match secs {
        s if s < 60 => return "just now".into(),
        s if s < 3600 => (s / 60, "m"),
        s if s < 86400 => (s / 3600, "h"),
        s if s < 86400 * 7 => (s / 86400, "d"),
        s if s < 86400 * 365 => (s / (86400 * 7), "w"),
        s => (s / (86400 * 365), "y"),
    };
    format!("{value}{unit} ago")
}

/// A count with its noun, singular at one: "1 album", "12 albums".
pub fn plural(n: u32, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tallies_read_singular_at_one() {
        assert_eq!(plural(0, "album"), "0 albums");
        assert_eq!(plural(1, "album"), "1 album");
        assert_eq!(plural(12, "track"), "12 tracks");
    }

    /// One unit, the largest that fits, and anything under a minute reads
    /// as now rather than as a number of seconds.
    #[test]
    fn ages_read_in_one_unit() {
        assert_eq!(fmt_ago(-5), "just now");
        assert_eq!(fmt_ago(59), "just now");
        assert_eq!(fmt_ago(60), "1m ago");
        assert_eq!(fmt_ago(3600), "1h ago");
        assert_eq!(fmt_ago(86400 * 3), "3d ago");
        assert_eq!(fmt_ago(86400 * 14), "2w ago");
        assert_eq!(fmt_ago(86400 * 365 * 2), "2y ago");
    }
}
