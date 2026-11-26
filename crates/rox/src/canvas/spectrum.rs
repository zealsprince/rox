//! The music's spectrum for renderers granted it (ADR 37): one shared
//! mapping rox writes and every such renderer reads. The trust direction is
//! the cover's. The renderer's handle can only read, so one mapping can be
//! lent to them all, and it lives as long as the app: it's made the first
//! time a granted session starts and never freed.
//!
//! It's written from the player's pump ticks on the UI thread, which is
//! where fresh audio lands in the feed, so a renderer gets the spectrum at
//! the hub's own rate whether or not its canvas paints. The FFT is the
//! feed's shared one at the hub's size, so the hub, a spectrum panel and
//! this pay for it once between them. Each session holds a [`Lease`]; with
//! none out a tick costs one atomic load. When nothing plays the mapping is
//! written once with zeros and the playing bit clear, then left alone.
//!
//! Magnitudes only, never samples and never phase: a full-rate waveform is
//! the audio itself, and a renderer could record what the user plays.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{App, Entity};
use rox_frames::spectrum::{self as mapping, Spectrum};
use rox_plugins::process;
use rox_services::player::{self, Player};
use rox_viz::AudioFeed;
use rox_viz::signal::{HUB_FFT, SILENT_AFTER};
use rox_viz::spectrum::Rows;

/// The handle every lease lends. A clone of the mapping's own, held apart
/// from the writer's lock so a spawn never waits on a write.
#[cfg(unix)]
type Owned = std::os::fd::OwnedFd;
#[cfg(windows)]
type Owned = std::os::windows::io::OwnedHandle;

/// The mapping and its writer, made once.
struct Shared {
    handle: Owned,
    leases: AtomicUsize,
    writer: Mutex<Writer>,
}

struct Writer {
    map: Spectrum,
    rows: Rows,
    /// The feed's `written` at the last tick.
    written: u64,
    /// When fresh audio last came.
    fresh_at: Option<Instant>,
    /// What the last write said.
    playing: bool,
}

static SHARED: OnceLock<Result<Shared, String>> = OnceLock::new();

/// One granted session's hold on the spectrum. The last one given back
/// stops the writes.
pub(crate) struct Lease {
    shared: &'static Shared,
}

/// A lease on the app's spectrum mapping, made the first time it's asked
/// for. None when it can't be made, which is logged once.
pub(crate) fn lease() -> Option<Lease> {
    let shared = SHARED.get_or_init(|| {
        Shared::create().inspect_err(|e| log::warn!("renderers: no spectrum mapping: {e}"))
    });

    shared.as_ref().ok().map(Shared::lease)
}

impl Lease {
    /// The read-only handle, for the spawn.
    #[cfg(unix)]
    pub(crate) fn lent(&self) -> process::Lent<'static> {
        use std::os::fd::AsFd;

        self.shared.handle.as_fd()
    }

    /// Never compiled: no Windows target builds here. Map read only.
    #[cfg(windows)]
    pub(crate) fn lent(&self) -> process::Lent<'static> {
        use std::os::windows::io::AsHandle;

        process::Lent {
            handle: self.shared.handle.as_handle(),
            access: mapping::CHILD_ACCESS,
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.shared.release();
    }
}

/// Writes the spectrum on each of `player`'s pump ticks while a lease is
/// out. Called once, beside the player it follows.
pub(crate) fn follow(player: &Entity<Player>, cx: &mut App) {
    player::observe_clock_app(player, cx, |player, cx| {
        let Some(Ok(shared)) = SHARED.get() else {
            return;
        };
        if shared.leases.load(Ordering::Acquire) == 0 {
            return;
        }

        let player = player.read(cx);
        shared.step(&player.feed(), player.is_playing(), Instant::now());
    });
}

impl Shared {
    fn create() -> Result<Shared, String> {
        let mut map = Spectrum::create().map_err(|e| e.to_string())?;
        let handle = map
            .child_handle()
            .try_clone_to_owned()
            .map_err(|e| e.to_string())?;
        if !map.read_only() {
            log::warn!("renderers: this kernel can't seal the spectrum read-only");
        }

        // Written once, so a renderer never finds it unwritten.
        map.write(None);

        Ok(Shared {
            handle,
            leases: AtomicUsize::new(0),
            writer: Mutex::new(Writer {
                map,
                rows: Rows::default(),
                written: 0,
                fresh_at: None,
                playing: false,
            }),
        })
    }

    fn lease(&'static self) -> Lease {
        self.leases.fetch_add(1, Ordering::AcqRel);
        Lease { shared: self }
    }

    /// The last lease back leaves zeros behind, so the next renderer to
    /// start doesn't read music that stopped long ago as playing.
    fn release(&self) {
        if self.leases.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.lock().quiet();
        }
    }

    fn step(&self, feed: &AudioFeed, playing: bool, now: Instant) {
        self.lock().step(feed, playing, now);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Writer> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Writer {
    /// A write for each fresh window of audio. Between windows the last
    /// one holds, as a pool band's value does. Paused, stopped, or silent
    /// past the hub's [`SILENT_AFTER`], it's zeros once.
    fn step(&mut self, feed: &AudioFeed, playing: bool, now: Instant) {
        let written = feed.written();
        let fresh = written != self.written;
        self.written = written;
        if fresh {
            self.fresh_at = Some(now);
        }

        let heard = self
            .fresh_at
            .is_some_and(|at| now.duration_since(at) <= Duration::from_secs_f32(SILENT_AFTER));
        if !playing || !heard {
            self.quiet();
            return;
        }
        if !fresh {
            return;
        }

        let Some(mags) = feed.magnitudes(HUB_FFT) else {
            return;
        };
        let sample_rate = feed.sample_rate();
        self.rows.fill(&mags, sample_rate);
        self.map.write(Some(mapping::Rows {
            log: self.rows.log(),
            linear: self.rows.linear(),
            sample_rate,
        }));
        self.playing = true;
    }

    fn quiet(&mut self) {
        if self.playing {
            self.map.write(None);
            self.playing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use rox_frames::spectrum::{Snapshot, SpectrumRead, SpectrumReader};

    use super::*;

    /// A mapping of the test's own, so tests don't share the app's.
    fn shared() -> &'static Shared {
        Box::leak(Box::new(Shared::create().unwrap()))
    }

    fn tone(feed: &AudioFeed, hz: f32, frames: usize) {
        let samples: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = 0.03 * (TAU * hz * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect();
        feed.push(&samples);
    }

    fn read(shared: &Shared) -> Snapshot {
        let reader =
            SpectrumReader::open(shared.writer.lock().unwrap().map.child_handle()).unwrap();
        let mut got = Snapshot::default();
        assert!(matches!(reader.read(&mut got), SpectrumRead::Whole(_)));
        got
    }

    #[test]
    fn the_layout_rox_frames_writes_is_the_one_the_rows_fill_and_hello_names() {
        use rox_services::renderers;
        use rox_viz::spectrum as rows;

        assert_eq!(rows::LOG_BINS, mapping::LOG_BINS);
        assert_eq!(rows::LINEAR_BINS, mapping::LINEAR_BINS);
        assert_eq!(rows::LOG_LOW_HZ, mapping::LOG_LOW_HZ);
        assert_eq!(rows::LOG_HIGH_HZ, mapping::LOG_HIGH_HZ);
        assert_eq!(rows::LINEAR_BIN_HZ, mapping::LINEAR_BIN_HZ);
        assert_eq!(rox_viz::signal::FLOOR_DB, mapping::FLOOR_DB);
        assert_eq!(rox_viz::signal::MAX_DB, mapping::MAX_DB);

        assert_eq!(renderers::SPECTRUM_BYTES as usize, mapping::BYTES);
        assert_eq!(renderers::SPECTRUM_LOG_BINS as usize, mapping::LOG_BINS);
        assert_eq!(
            renderers::SPECTRUM_LINEAR_BINS as usize,
            mapping::LINEAR_BINS
        );
        assert_eq!(renderers::SPECTRUM_RATE, mapping::RATE);
    }

    #[test]
    fn it_writes_while_a_lease_is_out_and_music_plays_and_rests_otherwise() {
        let shared = shared();
        let feed = AudioFeed::new();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);

        assert_eq!(read(shared).seq, 2, "written once at birth, as silence");

        let lease = shared.lease();
        tone(&feed, 1000.0, 2048);
        shared.step(&feed, true, at(0));
        let first = read(shared);
        assert!(first.playing);
        assert_eq!(first.seq, 4);
        assert_eq!(first.sample_rate, 48_000);
        let loudest = (0..first.log.len())
            .max_by(|&a, &b| first.log[a].total_cmp(&first.log[b]))
            .unwrap();
        assert_eq!(loudest, 72, "1 kHz is log bin 72");

        // No new audio: the last write holds and the counter stays.
        shared.step(&feed, true, at(16));
        assert_eq!(read(shared).seq, 4);

        // New audio: the next write.
        tone(&feed, 1000.0, 768);
        shared.step(&feed, true, at(32));
        assert_eq!(read(shared).seq, 6);

        // Silent past the hub's window: zeros once, then nothing.
        shared.step(&feed, true, at(250));
        let quiet = read(shared);
        assert_eq!(quiet.seq, 8);
        assert!(!quiet.playing);
        assert!(quiet.log.iter().all(|&v| v == 0.0));
        shared.step(&feed, true, at(266));
        shared.step(&feed, false, at(282));
        assert_eq!(read(shared).seq, 8, "an idle spectrum isn't written");

        // Music again: at once.
        tone(&feed, 1000.0, 768);
        shared.step(&feed, true, at(298));
        assert_eq!(read(shared).seq, 10);

        // A pause is zeros at once, whatever the feed holds.
        tone(&feed, 1000.0, 768);
        shared.step(&feed, false, at(314));
        let paused = read(shared);
        assert_eq!((paused.seq, paused.playing), (12, false));

        drop(lease);
        assert_eq!(shared.leases.load(Ordering::Acquire), 0);
    }

    #[test]
    fn the_last_lease_back_stops_it_and_leaves_zeros() {
        let shared = shared();
        let feed = AudioFeed::new();
        let now = Instant::now();

        let (one, two) = (shared.lease(), shared.lease());
        assert_eq!(shared.leases.load(Ordering::Acquire), 2);
        tone(&feed, 440.0, 2048);
        shared.step(&feed, true, now);
        assert!(read(shared).playing);

        drop(one);
        assert!(read(shared).playing, "a second session still reads it");

        drop(two);
        assert_eq!(shared.leases.load(Ordering::Acquire), 0);
        let left = read(shared);
        assert!(!left.playing && left.log.iter().all(|&v| v == 0.0));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn every_lease_lends_the_one_read_only_mapping() {
        use std::os::fd::AsRawFd;

        let shared = shared();
        let (one, two) = (shared.lease(), shared.lease());
        assert_eq!(one.lent().as_raw_fd(), two.lent().as_raw_fd());

        // SAFETY: an mmap of an fd the mapping holds open, unmapped at once
        // if it ever succeeds.
        let writable = unsafe {
            let map = libc::mmap(
                std::ptr::null_mut(),
                mapping::BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                one.lent().as_raw_fd(),
                0,
            );
            let mapped = map != libc::MAP_FAILED;
            if mapped {
                libc::munmap(map, mapping::BYTES);
            }
            mapped
        };
        assert!(!writable, "the lent handle can't map it writable");
        assert!(SpectrumReader::open(one.lent()).is_ok());
    }
}
