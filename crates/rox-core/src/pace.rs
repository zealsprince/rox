//! Rough time arithmetic for the long library passes: how fast a pass is
//! going, how long the rest should take, and how to say so like a person.
//!
//! Estimates only ever come from a pass measured on this machine. Never
//! estimate from baked-in constants: a laptop, a dev build, or a network
//! mount makes them wrong. No rate measured means no estimate shown.

use std::sync::Mutex;
use std::time::Instant;

/// Below either floor, the rate is one outlier: the first files carry the
/// model load and a cold page cache.
const MIN_DONE: usize = 3;
const MIN_SECS: f64 = 5.0;

#[derive(Default)]
pub struct Pace {
    /// Starts once the work list is ready, so DB scans and model loads don't
    /// bill the first track.
    started: Mutex<Option<Instant>>,
}

impl Pace {
    pub fn begin(&self) {
        *self.started.lock().unwrap() = Some(Instant::now());
    }

    pub fn secs_per_track(&self, done: usize) -> Option<f64> {
        let started = (*self.started.lock().unwrap())?;
        if done < MIN_DONE {
            return None;
        }
        let elapsed = started.elapsed().as_secs_f64();
        if elapsed < MIN_SECS {
            return None;
        }
        Some(elapsed / done as f64)
    }

    pub fn eta_secs(&self, done: usize, total: usize) -> Option<f64> {
        let per = self.secs_per_track(done)?;
        Some(per * total.saturating_sub(done) as f64)
    }
}

/// Enough that one long file doesn't set the number alone.
pub const PROBE_TRACKS: usize = 3;

/// Spread across the list, never off the front: a library's first tracks
/// are one album and often the shortest thing in the run.
pub fn sample_indices(len: usize, count: usize) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    let count = count.min(len);
    (0..count).map(|i| i * len / count).collect()
}

/// None when nothing is missing or nothing has been measured. The passes
/// are near linear in workers, so dividing by them is a fair answer.
pub fn estimate(pace: f32, missing: u64, workers: usize) -> Option<String> {
    if missing == 0 || pace <= 0.0 {
        return None;
    }
    Some(human(pace as f64 * missing as f64 / workers.max(1) as f64))
}

pub fn workers_phrase(workers: usize) -> String {
    rox_i18n::t!("pace-workers", count = workers as u64).to_string()
}

/// Precision shrinks as the number grows: "about 20 minutes", "about 2.5
/// hours", "about 3 days".
pub fn human(secs: f64) -> String {
    let minutes = secs / 60.0;
    if minutes < 1.0 {
        return rox_i18n::t!("pace-under-a-minute").to_string();
    }
    if minutes < 60.0 {
        return rox_i18n::t!("pace-minutes", count = minutes.round() as u64).to_string();
    }
    let hours = minutes / 60.0;
    if hours < 10.0 {
        // Halves under ten hours. 2.5 isn't a plural category any locale
        // names, so the half-hour case gets its own message.
        let halves = (hours * 2.0).round() / 2.0;
        return if halves.fract() == 0.0 {
            rox_i18n::t!("pace-hours", count = halves as u64).to_string()
        } else {
            rox_i18n::t!("pace-half-hours", value = halves).to_string()
        };
    }
    if hours < 48.0 {
        return rox_i18n::t!("pace-hours", count = hours.round() as u64).to_string();
    }
    rox_i18n::t!("pace-days", count = (hours / 24.0).round() as u64).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rate_before_work_or_off_a_sliver() {
        let pace = Pace::default();
        assert!(pace.secs_per_track(100).is_none(), "clock never started");
        pace.begin();
        assert!(pace.secs_per_track(0).is_none());
        assert!(
            pace.secs_per_track(MIN_DONE).is_none(),
            "five seconds haven't passed"
        );
        assert!(pace.eta_secs(MIN_DONE, 1000).is_none());
    }

    #[test]
    fn an_unmeasured_pass_is_not_estimated() {
        assert!(estimate(0.0, 5_000, 4).is_none(), "no pace measured");
        assert!(estimate(-1.0, 5_000, 4).is_none(), "nonsense pace");
        assert!(estimate(2.0, 0, 4).is_none(), "nothing missing");
    }

    /// Pinned in English under the shared lock, or it fails on a machine
    /// whose OS locale isn't English.
    #[test]
    fn workers_divide_the_wait() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        // 8 worker-seconds a track over 1,800 tracks: 4 hours on one worker.
        assert_eq!(estimate(8.0, 1_800, 1).unwrap(), "about 4 hours");
        assert_eq!(estimate(8.0, 1_800, 2).unwrap(), "about 2 hours");
        assert_eq!(estimate(8.0, 1_800, 4).unwrap(), "about an hour");
        assert_eq!(estimate(8.0, 1_800, 0), estimate(8.0, 1_800, 1));
        rox_i18n::set_locale(None);
    }

    #[test]
    fn a_sample_spreads_across_the_work_list() {
        assert_eq!(sample_indices(49_805, 3), vec![0, 16_601, 33_203]);
        assert_eq!(sample_indices(2, 3), vec![0, 1]);
        assert_eq!(sample_indices(1, 3), vec![0]);
        assert!(sample_indices(0, 3).is_empty());
        assert!(sample_indices(500, 0).is_empty());
        for len in [1usize, 2, 3, 7, 100, 5000] {
            let picked = sample_indices(len, PROBE_TRACKS);
            assert!(
                picked.iter().all(|&i| i < len),
                "index off the end at {len}"
            );
            let mut sorted = picked.clone();
            sorted.dedup();
            assert_eq!(sorted, picked, "a repeat would time one track twice");
        }
    }

    /// Locked: `workers_phrase` and `t!` read the global locale separately,
    /// and a sibling test can flip it between them.
    #[test]
    fn one_worker_is_singular() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        assert_eq!(
            workers_phrase(1),
            rox_i18n::t!("pace-workers", count = 1u64)
        );
        assert_eq!(
            workers_phrase(2),
            rox_i18n::t!("pace-workers", count = 2u64)
        );
        assert_eq!(
            workers_phrase(32),
            rox_i18n::t!("pace-workers", count = 32u64)
        );
        rox_i18n::set_locale(None);
    }

    #[test]
    fn durations_read_like_a_person_said_them() {
        assert_eq!(human(20.0), rox_i18n::t!("pace-under-a-minute"));
        assert_eq!(human(70.0), rox_i18n::t!("pace-minutes", count = 1u64));
        assert_eq!(
            human(60.0 * 20.0),
            rox_i18n::t!("pace-minutes", count = 20u64)
        );
        assert_eq!(human(3600.0), rox_i18n::t!("pace-hours", count = 1u64));
        assert_eq!(
            human(3600.0 * 2.4),
            rox_i18n::t!("pace-half-hours", value = 2.5)
        );
        assert_eq!(
            human(3600.0 * 3.1),
            rox_i18n::t!("pace-hours", count = 3u64)
        );
        assert_eq!(
            human(3600.0 * 11.6),
            rox_i18n::t!("pace-hours", count = 12u64)
        );
        assert_eq!(
            human(3600.0 * 72.0),
            rox_i18n::t!("pace-days", count = 3u64)
        );
    }
}
