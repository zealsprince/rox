//! The spectrum mapping: the music's frequency magnitudes, for a renderer
//! whose approved manifest asks for them (ADR 37). The trust direction is
//! the cover's. rox makes it, rox writes it, and the renderer only reads:
//! on Linux the memfd carries the future-write seal once rox's own view is
//! mapped. rox never reads the mapping back, and keeps its counter on its
//! own side.
//!
//! Magnitudes only, never samples and never phase. A full-rate waveform is
//! the audio itself, and a renderer could record what the user plays. Two
//! rows of normalised magnitudes with no phase can't give the recording
//! back, only a rough, vocoder-like sketch of it.
//!
//! One mapping serves every renderer granted the spectrum, since they all
//! want the same numbers and none can write them. Rows are only ever added
//! after the ones here, with a higher [`VERSION`], so every offset below
//! holds in a later layout.

use std::ptr;
use std::sync::atomic::{Ordering, fence};

use crate::Error;
use crate::layout::{store_u32, store_u64};
use crate::map::{Handle, os};

/// The header's first four bytes.
pub const MAGIC: [u8; 4] = *b"RXS1";
/// The layout version in the header.
pub const VERSION: u32 = 1;

/// Bins in the log row, log-spaced from [`LOG_LOW_HZ`] to [`LOG_HIGH_HZ`].
pub const LOG_BINS: usize = 128;
pub const LOG_LOW_HZ: f32 = 20.0;
pub const LOG_HIGH_HZ: f32 = 20_000.0;
/// Bins in the linear row, [`LINEAR_BIN_HZ`] apart from 0 Hz.
pub const LINEAR_BINS: usize = 512;
pub const LINEAR_BIN_HZ: f32 = 44_100.0 / 2048.0;
/// The dB window the values are normalised into: 0 at the floor, 1 at the
/// top.
pub const FLOOR_DB: f32 = -66.0;
pub const MAX_DB: f32 = -12.0;
/// The most writes a second: one per pass of the player's 16 ms pump.
pub const RATE: f32 = 62.5;

/// The header's size. The rows start here.
pub const HEADER_BYTES: usize = 128;
/// The whole mapping, a page.
pub const BYTES: usize = 4096;

/// Header field offsets.
pub mod header {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    /// u64: 0 never written, odd while rox writes, even once whole.
    pub const SEQ: usize = 8;
    /// u64: `CLOCK_MONOTONIC` in nanoseconds when the write finished, the
    /// clock a frame slot's stamp uses (`clock_ns`).
    pub const STAMP_NS: usize = 16;
    /// u32: bit 0 set while music plays.
    pub const FLAGS: usize = 24;
    /// u32: the rate of the audio the FFT ran on, 0 before any.
    pub const SAMPLE_RATE: usize = 28;
    /// f32: [`super::RATE`].
    pub const RATE: usize = 32;
    pub const LOG_BINS: usize = 36;
    pub const LOG_LOW_HZ: usize = 40;
    pub const LOG_HIGH_HZ: usize = 44;
    pub const LINEAR_BINS: usize = 48;
    pub const LINEAR_BIN_HZ: usize = 52;
    pub const FLOOR_DB: usize = 56;
    pub const MAX_DB: usize = 60;
    /// u32 each: where each row starts, from the start of the mapping.
    pub const LOG_AT: usize = 64;
    pub const LINEAR_AT: usize = 68;
    pub const LOG_BYTES_AT: usize = 72;
    pub const LINEAR_BYTES_AT: usize = 76;
}

/// Where each row starts: the two as f32, then the same two as u8.
pub mod rows {
    use super::{HEADER_BYTES, LINEAR_BINS, LOG_BINS};

    pub const LOG: usize = HEADER_BYTES;
    pub const LINEAR: usize = LOG + LOG_BINS * 4;
    pub const LOG_BYTES: usize = LINEAR + LINEAR_BINS * 4;
    pub const LINEAR_BYTES: usize = LOG_BYTES + LOG_BINS;
    /// Where the rows end.
    pub const END: usize = LINEAR_BYTES + LINEAR_BINS;
}

// The rows fit the page, and the header's fields fit before them.
const _: () = assert!(rows::END <= BYTES);
const _: () = assert!(header::LINEAR_BYTES_AT + 4 <= HEADER_BYTES);

/// The bit in the flags word set while music plays.
pub const PLAYING: u32 = 1;

/// The rights the renderer's copy of the section gets on Windows: map read,
/// and nothing else.
#[cfg(windows)]
pub const CHILD_ACCESS: u32 = os::COVER_CHILD_ACCESS;

/// One write's rows: values from 0 to 1, as the hub's dB window maps them.
pub struct Rows<'a> {
    pub log: &'a [f32; LOG_BINS],
    pub linear: &'a [f32; LINEAR_BINS],
    pub sample_rate: u32,
}

/// rox's side of the spectrum mapping: the handle renderers get, and the
/// one writable view.
pub struct Spectrum {
    // Field order is drop order: the view unmaps before the handle closes.
    view: os::WritableView,
    section: os::Section,
    /// The counter as rox last wrote it. Kept here and never read from the
    /// mapping.
    seq: u64,
    read_only: bool,
}

// SAFETY: the view is a raw region written only through `&mut self`, and the
// handle is a kernel object reference usable from any thread.
unsafe impl Send for Spectrum {}

impl Spectrum {
    /// Makes the mapping with its header written and its rows zero, counter
    /// 0.
    pub fn create() -> Result<Spectrum, Error> {
        let (section, mut view, read_only) = os::create_readable(BYTES, "rox-spectrum")?;
        let base = view.base_mut();

        // SAFETY: the view is BYTES long and page-aligned, which holds the
        // header, and the fields are aligned to their size.
        unsafe {
            ptr::copy_nonoverlapping(MAGIC.as_ptr(), base.add(header::MAGIC), MAGIC.len());
            store_u32(base, header::VERSION, VERSION);
            store_u64(base, header::SEQ, 0);
            store_f32(base, header::RATE, RATE);
            store_u32(base, header::LOG_BINS, LOG_BINS as u32);
            store_f32(base, header::LOG_LOW_HZ, LOG_LOW_HZ);
            store_f32(base, header::LOG_HIGH_HZ, LOG_HIGH_HZ);
            store_u32(base, header::LINEAR_BINS, LINEAR_BINS as u32);
            store_f32(base, header::LINEAR_BIN_HZ, LINEAR_BIN_HZ);
            store_f32(base, header::FLOOR_DB, FLOOR_DB);
            store_f32(base, header::MAX_DB, MAX_DB);
            store_u32(base, header::LOG_AT, rows::LOG as u32);
            store_u32(base, header::LINEAR_AT, rows::LINEAR as u32);
            store_u32(base, header::LOG_BYTES_AT, rows::LOG_BYTES as u32);
            store_u32(base, header::LINEAR_BYTES_AT, rows::LINEAR_BYTES as u32);
        }

        Ok(Spectrum {
            view,
            section,
            seq: 0,
            read_only,
        })
    }

    /// The handle to pass to a renderer. On Unix it's close-on-exec, so the
    /// spawn puts it in place on purpose. On Windows it's not inheritable,
    /// and the spawn duplicates it with [`CHILD_ACCESS`].
    pub fn child_handle(&self) -> Handle<'_> {
        self.section.borrow()
    }

    /// Whether a renderer's handle can only read: false on a Linux before
    /// 5.1, whose renderers could write numbers rox never reads back.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The counter of the last whole write, 0 before any.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Writes the rows while music plays, or zeros with the playing bit
    /// clear for None, between an odd and an even counter. Answers the even
    /// counter.
    pub fn write(&mut self, rows: Option<Rows<'_>>) -> u64 {
        let writing = self.seq.wrapping_add(1) | 1;
        let whole = writing.wrapping_add(1);
        let base = self.view.base_mut();

        // SAFETY: the counter is inside the header and 8-aligned.
        unsafe { store_u64(base, header::SEQ, writing) };

        // Keeps the odd counter ahead of every row.
        fence(Ordering::Release);

        let (flags, sample_rate) = match &rows {
            Some(rows) => (PLAYING, rows.sample_rate),
            None => (0, 0),
        };
        let value = |row: Option<&[f32]>, i: usize| row.map_or(0.0, |row| row[i].clamp(0.0, 1.0));
        let (log, linear) = match &rows {
            Some(rows) => (Some(&rows.log[..]), Some(&rows.linear[..])),
            None => (None, None),
        };

        // SAFETY: every store is below rows::END, inside the BYTES the view
        // maps, and aligned to its size. rox is the only writer: the
        // renderer's handle is sealed or opened read-only where the platform
        // can.
        unsafe {
            store_u32(base, header::FLAGS, flags);
            store_u32(base, header::SAMPLE_RATE, sample_rate);

            for i in 0..LOG_BINS {
                let v = value(log, i);
                store_f32(base, rows::LOG + i * 4, v);
                *base.add(rows::LOG_BYTES + i) = byte(v);
            }
            for i in 0..LINEAR_BINS {
                let v = value(linear, i);
                store_f32(base, rows::LINEAR + i * 4, v);
                *base.add(rows::LINEAR_BYTES + i) = byte(v);
            }

            store_u64(base, header::STAMP_NS, crate::clock_ns());
        }

        // Keeps every row ahead of the even counter.
        fence(Ordering::Release);

        // SAFETY: as the first store.
        unsafe { store_u64(base, header::SEQ, whole) };

        self.seq = whole;
        whole
    }
}

/// A value from 0 to 1 as the byte a texture upload takes.
fn byte(value: f32) -> u8 {
    (value * 255.0).round() as u8
}

/// # Safety
///
/// `base + at` is inside a live, writable mapping and 4-aligned.
unsafe fn store_f32(base: *mut u8, at: usize, value: f32) {
    // SAFETY: the caller's contract.
    unsafe { store_u32(base, at, value.to_bits()) }
}

#[cfg(any(test, feature = "cover-reader"))]
pub use reader::{Snapshot, SpectrumRead, SpectrumReader};

/// The renderer's side, behind the `cover-reader` feature, which rox's own
/// build never turns on. A reader trusts rox no more than rox trusts it: it
/// checks the size and header before it reads.
#[cfg(any(test, feature = "cover-reader"))]
mod reader {
    use std::sync::atomic::{Ordering, fence};

    use super::{BYTES, LINEAR_BINS, LOG_BINS, MAGIC, PLAYING, header, rows};
    use crate::Error;
    use crate::layout::{load_u32, load_u64};
    use crate::map::{Handle, os};

    /// A read-only view of the spectrum mapping.
    pub struct SpectrumReader {
        view: os::View,
    }

    // SAFETY: the view is only read, through raw pointers.
    unsafe impl Send for SpectrumReader {}

    /// One whole write, as read.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Snapshot {
        pub seq: u64,
        pub stamp_ns: u64,
        pub playing: bool,
        pub sample_rate: u32,
        pub log: [f32; LOG_BINS],
        pub linear: [f32; LINEAR_BINS],
        pub log_bytes: [u8; LOG_BINS],
        pub linear_bytes: [u8; LINEAR_BINS],
    }

    impl Default for Snapshot {
        fn default() -> Self {
            Snapshot {
                seq: 0,
                stamp_ns: 0,
                playing: false,
                sample_rate: 0,
                log: [0.0; LOG_BINS],
                linear: [0.0; LINEAR_BINS],
                log_bytes: [0; LOG_BINS],
                linear_bytes: [0; LINEAR_BINS],
            }
        }
    }

    /// What came of reading the spectrum.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SpectrumRead {
        /// The snapshot is one whole write, at this counter.
        Whole(u64),
        /// rox wrote it during the copy, or was writing when it began.
        Torn,
        /// Nothing written yet.
        Empty,
    }

    impl SpectrumReader {
        /// Maps `handle` read-only and checks it's a spectrum mapping of
        /// this version or a later one.
        pub fn open(handle: Handle<'_>) -> Result<SpectrumReader, Error> {
            let view = os::map_readable_prefix(handle, BYTES)?;
            let base = view.base();

            // SAFETY: the view is BYTES long, which holds the header. rox
            // may write the counter meanwhile, never the header around it.
            let matches = unsafe {
                std::ptr::read_volatile(base.add(header::MAGIC).cast::<[u8; 4]>()) == MAGIC
                    && load_u32(base, header::VERSION) >= 1
                    && load_u32(base, header::LOG_BINS) == LOG_BINS as u32
                    && load_u32(base, header::LINEAR_BINS) == LINEAR_BINS as u32
            };
            if !matches {
                return Err(Error::Mismatch);
            }

            Ok(SpectrumReader { view })
        }

        /// The counter now, without copying.
        pub fn seq(&self) -> u64 {
            // SAFETY: inside the header and 8-aligned.
            unsafe { load_u64(self.view.base(), header::SEQ) }
        }

        #[cfg(test)]
        pub(crate) fn view_base(&self) -> *const u8 {
            self.view.base()
        }

        /// Copies one write into `into`. Its contents mean something only on
        /// [`SpectrumRead::Whole`].
        pub fn read(&self, into: &mut Snapshot) -> SpectrumRead {
            self.read_between(into, || {})
        }

        /// `read` with a hook between the copy and the second counter read.
        pub(crate) fn read_between(
            &self,
            into: &mut Snapshot,
            between: impl FnOnce(),
        ) -> SpectrumRead {
            let base = self.view.base();

            let before = self.seq();
            if before == 0 {
                return SpectrumRead::Empty;
            }
            if before & 1 == 1 {
                return SpectrumRead::Torn;
            }

            fence(Ordering::Acquire);

            let mut bytes = [0u8; rows::END];
            // SAFETY: rows::END is inside the view, and `bytes` is that long.
            // rox may write the source as this copies it; the counter check
            // below throws such a copy away.
            unsafe { std::ptr::copy_nonoverlapping(base, bytes.as_mut_ptr(), rows::END) };

            between();
            fence(Ordering::Acquire);

            if self.seq() != before {
                return SpectrumRead::Torn;
            }

            let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
            let f32_at = |at: usize| f32::from_bits(u32_at(at));

            into.seq = before;
            into.stamp_ns = u64::from_le_bytes(
                bytes[header::STAMP_NS..header::STAMP_NS + 8]
                    .try_into()
                    .unwrap(),
            );
            into.playing = u32_at(header::FLAGS) & PLAYING != 0;
            into.sample_rate = u32_at(header::SAMPLE_RATE);
            for (i, v) in into.log.iter_mut().enumerate() {
                *v = f32_at(rows::LOG + i * 4);
            }
            for (i, v) in into.linear.iter_mut().enumerate() {
                *v = f32_at(rows::LINEAR + i * 4);
            }
            into.log_bytes
                .copy_from_slice(&bytes[rows::LOG_BYTES..rows::LOG_BYTES + LOG_BINS]);
            into.linear_bytes
                .copy_from_slice(&bytes[rows::LINEAR_BYTES..rows::LINEAR_BYTES + LINEAR_BINS]);

            SpectrumRead::Whole(before)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp<const N: usize>() -> [f32; N] {
        std::array::from_fn(|i| i as f32 / (N - 1) as f32)
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        assert_eq!(rows::LOG, 128);
        assert_eq!(rows::LINEAR, 640);
        assert_eq!(rows::LOG_BYTES, 2688);
        assert_eq!(rows::LINEAR_BYTES, 2816);
        assert_eq!(rows::END, 3328);
        assert_eq!(header::SEQ % 8, 0);
        assert_eq!(header::STAMP_NS % 8, 0);
    }

    #[test]
    fn what_rox_writes_is_what_a_renderer_reads() {
        let mut spectrum = Spectrum::create().unwrap();
        let reader = SpectrumReader::open(spectrum.child_handle()).unwrap();
        let mut got = Snapshot::default();

        assert_eq!(reader.read(&mut got), SpectrumRead::Empty, "unwritten");

        let (log, linear) = (ramp::<LOG_BINS>(), ramp::<LINEAR_BINS>());
        let before = crate::clock_ns();
        let seq = spectrum.write(Some(Rows {
            log: &log,
            linear: &linear,
            sample_rate: 48_000,
        }));
        assert_eq!(seq, 2);
        assert_eq!(reader.read(&mut got), SpectrumRead::Whole(2));
        assert!(got.playing);
        assert_eq!(got.sample_rate, 48_000);
        assert_eq!(got.log, log);
        assert_eq!(got.linear, linear);
        assert_eq!((got.log_bytes[0], got.log_bytes[LOG_BINS - 1]), (0, 255));
        assert_eq!(got.linear_bytes[LINEAR_BINS / 2], 128, "0.5 rounds up");
        assert!(got.stamp_ns >= before, "the same clock as a slot's stamp");

        assert_eq!(spectrum.write(None), 4, "silence is a write too");
        assert_eq!(reader.read(&mut got), SpectrumRead::Whole(4));
        assert!(!got.playing);
        assert_eq!(got.sample_rate, 0);
        assert!(got.log.iter().chain(&got.linear).all(|&v| v == 0.0));
        assert!(
            got.log_bytes
                .iter()
                .chain(&got.linear_bytes)
                .all(|&b| b == 0)
        );
    }

    #[test]
    fn the_header_says_what_the_rows_are() {
        use crate::layout::load_u32;

        let spectrum = Spectrum::create().unwrap();
        let reader = SpectrumReader::open(spectrum.child_handle()).unwrap();
        let base = reader_base(&reader);

        // SAFETY: every offset is inside the header the reader checked.
        let (version, log_bins, linear_bins, at, rate, low, high, bin_hz, floor, top) = unsafe {
            let f = |at| f32::from_bits(load_u32(base, at));
            (
                load_u32(base, header::VERSION),
                load_u32(base, header::LOG_BINS),
                load_u32(base, header::LINEAR_BINS),
                [
                    load_u32(base, header::LOG_AT),
                    load_u32(base, header::LINEAR_AT),
                    load_u32(base, header::LOG_BYTES_AT),
                    load_u32(base, header::LINEAR_BYTES_AT),
                ],
                f(header::RATE),
                f(header::LOG_LOW_HZ),
                f(header::LOG_HIGH_HZ),
                f(header::LINEAR_BIN_HZ),
                f(header::FLOOR_DB),
                f(header::MAX_DB),
            )
        };

        assert_eq!((version, log_bins, linear_bins), (1, 128, 512));
        assert_eq!(at, [128, 640, 2688, 2816]);
        assert_eq!((rate, low, high), (62.5, 20.0, 20_000.0));
        assert_eq!((bin_hz, floor, top), (21.533_203, -66.0, -12.0));
    }

    fn reader_base(reader: &SpectrumReader) -> *const u8 {
        reader.view_base()
    }

    #[test]
    fn a_read_rox_writes_under_is_torn() {
        let mut spectrum = Spectrum::create().unwrap();
        let reader = SpectrumReader::open(spectrum.child_handle()).unwrap();
        let mut got = Snapshot::default();

        spectrum.write(None);
        let read = reader.read_between(&mut got, || {
            spectrum.write(None);
        });
        assert_eq!(read, SpectrumRead::Torn, "the counter moved under the copy");
        assert_eq!(reader.read(&mut got), SpectrumRead::Whole(4));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_renderer_can_only_read_it() {
        use std::os::fd::AsRawFd;

        let spectrum = Spectrum::create().unwrap();
        assert!(spectrum.read_only(), "a kernel from 5.1 on seals it");
        let fd = spectrum.child_handle().as_raw_fd();

        // SAFETY: mmap, pwrite, ftruncate and fcntl on an fd the mapping
        // holds open. A mapping that succeeds is unmapped before the asserts.
        let (shared_write, write, shrink, seals) = unsafe {
            let map = libc::mmap(
                std::ptr::null_mut(),
                BYTES,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            let mapped = map != libc::MAP_FAILED;
            if mapped {
                libc::munmap(map, BYTES);
            }
            let byte = 1u8;
            (
                mapped,
                libc::pwrite(fd, (&byte as *const u8).cast(), 1, 0),
                libc::ftruncate(fd, 0),
                libc::fcntl(fd, libc::F_GET_SEALS),
            )
        };

        assert!(!shared_write, "a writable shared mapping is refused");
        assert_eq!(write, -1, "so is a write");
        assert_eq!(shrink, -1);
        let want =
            libc::F_SEAL_FUTURE_WRITE | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        assert_eq!(seals & want, want);

        // And a read-only one works.
        assert!(SpectrumReader::open(spectrum.child_handle()).is_ok());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_spectrum_has_its_own_name() {
        use std::os::fd::AsRawFd;

        let spectrum = Spectrum::create().unwrap();
        let named = std::fs::read_link(format!(
            "/proc/self/fd/{}",
            spectrum.child_handle().as_raw_fd()
        ))
        .unwrap();
        assert!(named.to_string_lossy().starts_with("/memfd:rox-spectrum"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_cover_is_no_spectrum() {
        let cover = crate::cover::Cover::create().unwrap();
        assert!(matches!(
            SpectrumReader::open(cover.child_handle()),
            Err(Error::Mismatch)
        ));
    }
}
