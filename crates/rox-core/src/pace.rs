//! Rough time arithmetic for the library passes: how fast a pass is going,
//! how long the rest should take, and how to say so like a person.
//!
//! Both long passes (ReplayGain measurement, acoustic analysis) share the
//! shape: a work list of tracks, a counter the workers bump, an afternoon of
//! wall time nobody can see the end of from a bare "132 of 41,000". This
//! module is the missing end: a [`Pace`] embedded in a pass's progress turns
//! the counter into "about 2 hours left", and a rate persisted from the last
//! pass (`SessionState`) prices the next one before it starts.
//!
//! Everything here is rough and says so in its wording. The only source is
//! the pass itself, measured on this machine over these files; nothing is
//! estimated from baked-in constants, because a laptop, a dev build, and a
//! network mount would each make a liar of them. No rate measured yet means
//! no estimate shown.

use std::sync::Mutex;
use std::time::Instant;

/// Below this many finished tracks, or this many seconds, a rate is one
/// outlier rather than a trend: the first files of a pass include the
/// model load and the cold page cache, and one long album track would set
/// the pace for a library. Past both, the average is worth repeating.
const MIN_DONE: usize = 3;
const MIN_SECS: f64 = 5.0;

/// A stopwatch over a counted pass. Owns only the clock; the done and total
/// counts stay in the pass's own progress, which hands them in per call.
#[derive(Default)]
pub struct Pace {
    /// When the work list was ready and real work began. None until then,
    /// so time spent scanning the database or loading a model doesn't bill
    /// the first track.
    started: Mutex<Option<Instant>>,
}

impl Pace {
    /// Start the clock. Called when the work list is built rather than when
    /// the pass is created; everything before that is overhead that
    /// shouldn't count against the tracks.
    pub fn begin(&self) {
        *self.started.lock().unwrap() = Some(Instant::now());
    }

    /// Seconds each finished track has cost so far, or None until enough
    /// have finished to mean anything.
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

    /// Seconds the rest of the pass should take at the rate so far.
    pub fn eta_secs(&self, done: usize, total: usize) -> Option<f64> {
        let per = self.secs_per_track(done)?;
        Some(per * total.saturating_sub(done) as f64)
    }
}

/// How many tracks a probe times to learn a machine's pace. Enough that one
/// unusually long file doesn't set the number alone, few enough that the
/// wait for an estimate is seconds rather than a pass of its own.
pub const PROBE_TRACKS: usize = 3;

/// Which items to time when sampling a work list of `len` for a pace, spread
/// across it rather than taken off the front.
///
/// A library's first few tracks are one album, by one artist, at one
/// bitrate, in one format, and often the shortest thing in the run. Timing
/// those and calling it the library's pace is how an estimate ends up off by
/// a factor rather than a margin.
pub fn sample_indices(len: usize, count: usize) -> Vec<usize> {
    if len == 0 || count == 0 {
        return Vec::new();
    }
    let count = count.min(len);
    (0..count).map(|i| i * len / count).collect()
}

/// A rough cost for `missing` tracks at `workers`, off a measured pace in
/// worker-seconds per track. None when there's nothing to do or nothing has
/// been measured yet: an estimate off baked-in constants would be wrong on
/// every machine but whichever one they were taken from, so nothing is
/// shown until a pass has run here.
///
/// Worker-seconds makes the prompt's slider live: the passes are
/// close enough to linear in workers that dividing is a fair answer to
/// "what would twice as many buy me", which is the only question the slider
/// is being asked.
pub fn estimate(pace: f32, missing: u64, workers: usize) -> Option<String> {
    if missing == 0 || pace <= 0.0 {
        return None;
    }
    Some(human(pace as f64 * missing as f64 / workers.max(1) as f64))
}

/// "4 workers", or "1 worker".
pub fn workers_phrase(workers: usize) -> String {
    rox_i18n::t!("pace-workers", count = workers as u64).to_string()
}

/// A duration as a person would say it, hedged to match how rough it is:
/// "under a minute", "about 20 minutes", "about 2.5 hours", "about 3 days".
/// The precision shrinks as the number grows, because a pass that runs
/// all day can't promise the half hour.
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
        // Halves under ten hours: "about 2.5 hours" reads as the estimate
        // it is, where "about 152 minutes" would read as a measurement.
        // The half-hour case can't go through a plural selector, since
        // Fluent selects on the number and 2.5 is not a plural category
        // any locale names; it gets its own message.
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

    /// The clock doesn't run until the pass starts it, and too small a sample
    /// stays quiet rather than projecting a library from one track.
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

    /// Nothing measured means nothing claimed. The alternative, a guess off
    /// a constant, would be confidently wrong on every machine that isn't
    /// the one it was taken from, and a wrong ETA is worse than none.
    #[test]
    fn an_unmeasured_pass_is_not_estimated() {
        assert!(estimate(0.0, 5_000, 4).is_none(), "no pace measured");
        assert!(estimate(-1.0, 5_000, 4).is_none(), "nonsense pace");
        assert!(estimate(2.0, 0, 4).is_none(), "nothing missing");
    }

    /// The slider's whole promise: twice the workers, half the wait. Worth
    /// pinning because someone commits an afternoon against the readout.
    ///
    /// Pinned in English under the shared lock. The wording is in the
    /// locale files now, so without this the assertions below would read
    /// whatever the OS locale negotiated to and fail on a German machine.
    #[test]
    fn workers_divide_the_wait() {
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        // 8 worker-seconds a track over 1,800 tracks: 4 hours on one worker.
        assert_eq!(estimate(8.0, 1_800, 1).unwrap(), "about 4 hours");
        assert_eq!(estimate(8.0, 1_800, 2).unwrap(), "about 2 hours");
        assert_eq!(estimate(8.0, 1_800, 4).unwrap(), "about an hour");
        // A zero count can't divide by nothing, and reads as one worker.
        assert_eq!(estimate(8.0, 1_800, 0), estimate(8.0, 1_800, 1));
        rox_i18n::set_locale(None);
    }

    /// A sample spreads across the work list, never just its head, and never
    /// asks for an item that isn't there.
    #[test]
    fn a_sample_spreads_across_the_work_list() {
        assert_eq!(sample_indices(49_805, 3), vec![0, 16_601, 33_203]);
        // Fewer items than the sample wants takes each of them once.
        assert_eq!(sample_indices(2, 3), vec![0, 1]);
        assert_eq!(sample_indices(1, 3), vec![0]);
        assert!(sample_indices(0, 3).is_empty());
        assert!(sample_indices(500, 0).is_empty());
        // Whatever the shape, every index is real and they never repeat.
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

    /// The one place the count is spelled out, so "1 workers" never ships.
    #[test]
    fn one_worker_is_singular() {
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
    }

    /// The wording holds its shape across the scales a pass actually spans,
    /// and hedges harder the longer it gets.
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
