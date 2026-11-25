//! The cover mapping: the playing track's cover as pixels, for a renderer
//! whose approved manifest asks for it (ADR 37). The trust direction is the
//! frame mapping's reversed. rox makes it, rox writes it, and the renderer
//! only reads. On Linux the memfd carries the future-write seal once rox's
//! own view is mapped, so the renderer's fd maps read-only and can't be
//! written through. rox never reads the mapping back: the counter it bumps
//! is kept in rox, so nothing the renderer could do to the bytes reaches it.
//!
//! The square is a fixed [`SIZE`] by [`SIZE`], RGBA8, straight alpha, sRGB,
//! top row first. A cover that isn't square is scaled to fit and centred,
//! with the rest transparent. A torn read shows the way a frame slot's does:
//! the counter is odd while rox writes, and moves when a write lands.

use std::ptr;
use std::sync::atomic::{Ordering, fence};

use crate::Error;
use crate::layout::{PAGE, PIXEL, store_u32, store_u64};
use crate::map::{Handle, os};

/// The header's first four bytes.
pub const MAGIC: [u8; 4] = *b"RXC1";
/// The layout version in the header.
pub const VERSION: u32 = 1;
/// Pixels across and down.
pub const SIZE: u32 = 256;
/// Bytes per row, `SIZE * 4`.
pub const STRIDE: u32 = SIZE * PIXEL;
/// Bytes of pixels.
pub const PIXEL_BYTES: usize = (STRIDE * SIZE) as usize;
/// The whole mapping: the header and the pixels, rounded up to the layout's
/// page.
pub const BYTES: usize = (HEADER_BYTES as u64 + PIXEL_BYTES as u64).next_multiple_of(PAGE) as usize;

/// Where the pixels start.
pub const HEADER_BYTES: usize = 64;

/// Header field offsets.
pub mod header {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const WIDTH: usize = 8;
    pub const HEIGHT: usize = 12;
    /// u64: 0 never written, odd while rox writes, even once a cover is whole.
    pub const SEQ: usize = 16;
    pub const PIXELS: usize = super::HEADER_BYTES;
}

/// The rights the renderer's copy of the section gets on Windows: map read,
/// and nothing else.
#[cfg(windows)]
pub const CHILD_ACCESS: u32 = os::COVER_CHILD_ACCESS;

/// Where a picture of this size lands scaled to fit the square, as
/// `[x, y, width, height]`. None for an empty picture.
pub fn fit(width: u32, height: u32) -> Option<[u32; 4]> {
    if width == 0 || height == 0 {
        return None;
    }

    let long = width.max(height) as f64;
    let scale = |side: u32| ((side as f64 * SIZE as f64 / long).round() as u32).clamp(1, SIZE);
    let (w, h) = (scale(width), scale(height));

    Some([(SIZE - w) / 2, (SIZE - h) / 2, w, h])
}

/// The square's pixels with `rgba`, already scaled to `width` by `height`
/// as [`fit`] says, placed where [`fit`] puts it, and transparent around
/// it. None when `rgba` isn't that size or the size doesn't fit.
pub fn letterbox(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > SIZE || height > SIZE {
        return None;
    }
    let row = (width * PIXEL) as usize;
    if rgba.len() != row * height as usize {
        return None;
    }

    let (x, y) = ((SIZE - width) / 2, (SIZE - height) / 2);
    let mut square = vec![0u8; PIXEL_BYTES];
    for (line, pixels) in rgba.chunks_exact(row).enumerate() {
        let at = (y as usize + line) * STRIDE as usize + (x * PIXEL) as usize;
        square[at..at + row].copy_from_slice(pixels);
    }

    Some(square)
}

/// rox's side of one session's cover mapping: the handle the renderer
/// gets, and the one writable view.
pub struct Cover {
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
unsafe impl Send for Cover {}

impl Cover {
    /// Makes the mapping with its header written and its pixels transparent,
    /// counter 0.
    pub fn create() -> Result<Cover, Error> {
        let (section, mut view, read_only) = os::create_cover(BYTES)?;
        let base = view.base_mut();

        // SAFETY: the view is BYTES long and page-aligned, which holds the
        // header, and the fields are aligned to their size.
        unsafe {
            ptr::copy_nonoverlapping(MAGIC.as_ptr(), base.add(header::MAGIC), MAGIC.len());
            store_u32(base, header::VERSION, VERSION);
            store_u32(base, header::WIDTH, SIZE);
            store_u32(base, header::HEIGHT, SIZE);
            store_u64(base, header::SEQ, 0);
        }

        Ok(Cover {
            view,
            section,
            seq: 0,
            read_only,
        })
    }

    /// The handle to pass to the renderer. On Unix it's close-on-exec, so
    /// the spawn puts it in place on purpose. On Windows it's not
    /// inheritable, and the spawn duplicates it with [`CHILD_ACCESS`].
    pub fn child_handle(&self) -> Handle<'_> {
        self.section.borrow()
    }

    /// Whether the renderer's handle can only read: true wherever the
    /// platform can seal or open it so. False on a Linux before 5.1, whose
    /// renderer could write the pixels it's sent and nothing more.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The counter of the last whole write, 0 before any.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Writes a cover, or transparent pixels for None or for pixels that
    /// aren't [`PIXEL_BYTES`] long, between an odd and an even counter.
    /// Answers the even counter.
    pub fn write(&mut self, pixels: Option<&[u8]>) -> u64 {
        let pixels = pixels.filter(|pixels| pixels.len() == PIXEL_BYTES);
        let writing = self.seq.wrapping_add(1) | 1;
        let whole = writing.wrapping_add(1);
        let base = self.view.base_mut();

        // SAFETY: the counter is inside the header and 8-aligned.
        unsafe { store_u64(base, header::SEQ, writing) };

        // Keeps the odd counter ahead of every pixel.
        fence(Ordering::Release);

        // SAFETY: the pixels run PIXEL_BYTES from header::PIXELS, inside the
        // BYTES the view maps. rox is the only writer: the renderer's handle
        // is sealed or opened read-only where the platform can.
        unsafe {
            let at = base.add(header::PIXELS);
            match pixels {
                Some(pixels) => ptr::copy_nonoverlapping(pixels.as_ptr(), at, PIXEL_BYTES),
                None => ptr::write_bytes(at, 0, PIXEL_BYTES),
            }
        }

        // Keeps every pixel ahead of the even counter.
        fence(Ordering::Release);

        // SAFETY: as the first store.
        unsafe { store_u64(base, header::SEQ, whole) };

        self.seq = whole;
        whole
    }
}

#[cfg(any(test, feature = "cover-reader"))]
pub use reader::{CoverRead, CoverReader};

/// The renderer's side, behind the `cover-reader` feature, which rox's own
/// build never turns on. A reader trusts rox no more than rox trusts it: it
/// checks the size and header before it reads.
#[cfg(any(test, feature = "cover-reader"))]
mod reader {
    use std::sync::atomic::{Ordering, fence};

    use super::{BYTES, MAGIC, PIXEL_BYTES, SIZE, VERSION, header};
    use crate::Error;
    use crate::layout::{load_u32, load_u64};
    use crate::map::{Handle, os};

    /// A read-only view of a cover mapping.
    pub struct CoverReader {
        view: os::View,
    }

    // SAFETY: the view is only read, through raw pointers.
    unsafe impl Send for CoverReader {}

    /// What came of reading the cover.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CoverRead {
        /// The copy is one whole cover, at this counter.
        Whole(u64),
        /// rox wrote it during the copy, or was writing when it began.
        Torn,
        /// Nothing written yet.
        Empty,
    }

    impl CoverReader {
        /// Maps `handle` read-only and checks it's a cover mapping.
        pub fn open(handle: Handle<'_>) -> Result<CoverReader, Error> {
            let view = os::map_readable(handle, BYTES)?;
            let base = view.base();

            // SAFETY: the view is BYTES long, which holds the header. rox
            // may write the counter meanwhile, never the header around it.
            let matches = unsafe {
                std::ptr::read_volatile(base.add(header::MAGIC).cast::<[u8; 4]>()) == MAGIC
                    && load_u32(base, header::VERSION) == VERSION
                    && load_u32(base, header::WIDTH) == SIZE
                    && load_u32(base, header::HEIGHT) == SIZE
            };
            if !matches {
                return Err(Error::Mismatch);
            }

            Ok(CoverReader { view })
        }

        /// The counter now, without copying.
        pub fn seq(&self) -> u64 {
            // SAFETY: inside the header and 8-aligned.
            unsafe { load_u64(self.view.base(), header::SEQ) }
        }

        /// Copies the pixels into `into`, which ends up [`PIXEL_BYTES`]
        /// long. Its contents mean something only on [`CoverRead::Whole`].
        pub fn read(&self, into: &mut Vec<u8>) -> CoverRead {
            self.read_between(into, || {})
        }

        /// `read` with a hook between the copy and the second counter read.
        pub(crate) fn read_between(&self, into: &mut Vec<u8>, between: impl FnOnce()) -> CoverRead {
            let base = self.view.base();

            let before = self.seq();
            if before == 0 {
                return CoverRead::Empty;
            }
            if before & 1 == 1 {
                return CoverRead::Torn;
            }

            fence(Ordering::Acquire);

            into.resize(PIXEL_BYTES, 0);
            // SAFETY: PIXEL_BYTES from header::PIXELS is inside the view, and
            // `into` was just sized to it. rox may write the source as this
            // copies it; the counter check below throws such a copy away.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    base.add(header::PIXELS),
                    into.as_mut_ptr(),
                    PIXEL_BYTES,
                );
            }

            between();
            fence(Ordering::Acquire);

            match self.seq() {
                after if after == before => CoverRead::Whole(before),
                _ => CoverRead::Torn,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_the_documented_one() {
        assert_eq!(STRIDE, 1024);
        assert_eq!(PIXEL_BYTES, 262_144);
        assert_eq!(BYTES, 266_240, "64 + 262144, up to a multiple of 4096");
        assert_eq!(BYTES % PAGE as usize, 0);
        assert_eq!(header::SEQ % 8, 0);
        assert_eq!(header::PIXELS, 64);
    }

    #[test]
    fn a_picture_fits_centred_and_keeps_its_shape() {
        assert_eq!(fit(256, 256), Some([0, 0, 256, 256]));
        assert_eq!(fit(1000, 1000), Some([0, 0, 256, 256]), "scaled down");
        assert_eq!(fit(100, 100), Some([0, 0, 256, 256]), "scaled up");
        assert_eq!(
            fit(512, 256),
            Some([0, 64, 256, 128]),
            "wide: bands above and below"
        );
        assert_eq!(
            fit(100, 300),
            Some([85, 0, 85, 256]),
            "tall: bands left and right"
        );
        assert_eq!(
            fit(10_000, 1),
            Some([0, 127, 256, 1]),
            "never thinner than a pixel"
        );
        assert_eq!(fit(0, 10), None);
    }

    #[test]
    fn a_letterboxed_cover_is_transparent_around_it() {
        let [x, y, w, h] = fit(512, 256).unwrap();
        let red: Vec<u8> = [255, 0, 0, 255].repeat((w * h) as usize);
        let square = letterbox(&red, w, h).unwrap();
        assert_eq!(square.len(), PIXEL_BYTES);

        let at = |px: u32, py: u32| {
            let i = (py * STRIDE + px * PIXEL) as usize;
            [square[i], square[i + 1], square[i + 2], square[i + 3]]
        };
        assert_eq!(at(0, 0), [0, 0, 0, 0], "the band above");
        assert_eq!(at(128, y - 1), [0, 0, 0, 0]);
        assert_eq!(at(x, y), [255, 0, 0, 255], "the picture's first pixel");
        assert_eq!(at(255, y + h - 1), [255, 0, 0, 255], "its last");
        assert_eq!(at(128, y + h), [0, 0, 0, 0], "the band below");

        assert_eq!(
            letterbox(&red, w, h + 1),
            None,
            "the size has to be the pixels'"
        );
        assert_eq!(letterbox(&[], 0, 0), None);
    }

    #[test]
    fn what_rox_writes_is_what_a_renderer_reads() {
        let mut cover = Cover::create().unwrap();
        let reader = CoverReader::open(cover.child_handle()).unwrap();
        let mut got = Vec::new();

        assert_eq!(reader.read(&mut got), CoverRead::Empty, "unwritten");

        let blue: Vec<u8> = [0, 0, 255, 255].repeat(PIXEL_BYTES / 4);
        let seq = cover.write(Some(&blue));
        assert_eq!(seq, 2);
        assert_eq!(reader.read(&mut got), CoverRead::Whole(2));
        assert_eq!(got, blue);

        assert_eq!(cover.write(None), 4, "a missing cover is a write too");
        assert_eq!(reader.read(&mut got), CoverRead::Whole(4));
        assert!(got.iter().all(|&b| b == 0), "transparent");

        assert_eq!(
            cover.write(Some(&blue[1..])),
            6,
            "the wrong size is no cover"
        );
        assert_eq!(reader.read(&mut got), CoverRead::Whole(6));
        assert!(got.iter().all(|&b| b == 0));
    }

    #[test]
    fn a_read_rox_writes_under_is_torn() {
        let mut cover = Cover::create().unwrap();
        let reader = CoverReader::open(cover.child_handle()).unwrap();
        let mut got = Vec::new();

        let white = vec![255u8; PIXEL_BYTES];
        cover.write(Some(&white));

        let read = reader.read_between(&mut got, || {
            cover.write(None);
        });
        assert_eq!(read, CoverRead::Torn, "the counter moved under the copy");
        assert_eq!(reader.read(&mut got), CoverRead::Whole(4));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_renderer_can_only_read_it() {
        use std::os::fd::AsRawFd;

        let cover = Cover::create().unwrap();
        assert!(cover.read_only(), "a kernel from 5.1 on seals it");
        let fd = cover.child_handle().as_raw_fd();

        // SAFETY: mmap, pwrite, ftruncate and fcntl on an fd the cover holds
        // open. A mapping that succeeds is unmapped before the asserts.
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
        assert!(CoverReader::open(cover.child_handle()).is_ok());
    }
}
