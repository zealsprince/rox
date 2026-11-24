//! The mapping's byte layout, and the one place its sizes are computed.
//! Every offset rox uses inside the mapping comes from a [`Spec`], which
//! is built from the size rox asked for and nothing the renderer wrote.

use std::ptr;

use crate::Error;

/// The header's first four bytes.
pub const MAGIC: [u8; 4] = *b"RXF1";
/// The layout version in the header.
pub const VERSION: u32 = 1;
/// The header's size. The first slot starts here.
pub const HEADER_BYTES: usize = 4096;
/// How many slots follow the header.
pub const SLOTS: u32 = 2;
/// A slot's size is rounded up to this. It's the layout's page, fixed on
/// every platform, not the OS page size.
pub const PAGE: u64 = 4096;
/// Bytes per pixel: RGBA8.
pub const PIXEL: u32 = 4;

/// The narrowest and widest a target draws.
pub const WIDTH: std::ops::RangeInclusive<u32> = 16..=3840;
/// The shortest and tallest a target draws.
pub const HEIGHT: std::ops::RangeInclusive<u32> = 16..=2160;

/// Header field offsets.
pub mod header {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const WIDTH: usize = 8;
    pub const HEIGHT: usize = 12;
    pub const STRIDE: usize = 16;
    pub const SLOTS: usize = 20;
    pub const SLOT_BYTES: usize = 24;
}

/// Field offsets inside a slot.
pub mod slot {
    pub const SEQ: usize = 0;
    pub const STAMP_NS: usize = 8;
    pub const WIDTH: usize = 16;
    pub const HEIGHT: usize = 20;
    pub const PIXELS: usize = 64;
}

/// The size of one session's mapping, and every size derived from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    width: u32,
    height: u32,
    stride: u32,
    pixel_bytes: usize,
    slot_bytes: usize,
    slot_at: [usize; SLOTS as usize],
    bytes: usize,
}

impl Spec {
    /// A spec for a target of this size, clamped to [`WIDTH`] by
    /// [`HEIGHT`].
    pub fn new(width: u32, height: u32) -> Result<Spec, Error> {
        Spec::exact(
            width.clamp(*WIDTH.start(), *WIDTH.end()),
            height.clamp(*HEIGHT.start(), *HEIGHT.end()),
        )
    }

    /// The same arithmetic with no clamp, so overflow stays testable.
    pub(crate) fn exact(width: u32, height: u32) -> Result<Spec, Error> {
        let stride = width.checked_mul(PIXEL).ok_or(Error::Overflow)?;
        let pixels = u64::from(stride)
            .checked_mul(u64::from(height))
            .ok_or(Error::Overflow)?;

        let slot = pixels
            .checked_add(slot::PIXELS as u64)
            .and_then(|n| n.checked_next_multiple_of(PAGE))
            .ok_or(Error::Overflow)?;
        let second = slot
            .checked_add(HEADER_BYTES as u64)
            .ok_or(Error::Overflow)?;
        let total = slot
            .checked_mul(u64::from(SLOTS))
            .and_then(|n| n.checked_add(HEADER_BYTES as u64))
            .ok_or(Error::Overflow)?;

        // A 32-bit build can hold the u64 sums and still not address them.
        let size = |n: u64| usize::try_from(n).map_err(|_| Error::Overflow);
        let slot_bytes = size(slot)?;

        Ok(Spec {
            width,
            height,
            stride,
            pixel_bytes: size(pixels)?,
            slot_bytes,
            slot_at: [HEADER_BYTES, size(second)?],
            bytes: size(total)?,
        })
    }

    /// Pixels across.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Pixels down.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per row, `width * 4`.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Bytes in one frame, `stride * height`.
    pub fn pixel_bytes(&self) -> usize {
        self.pixel_bytes
    }

    /// Bytes in one slot, its header included, rounded up to [`PAGE`].
    pub fn slot_bytes(&self) -> usize {
        self.slot_bytes
    }

    /// Bytes in the whole mapping.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Where a slot starts in the mapping, or None past the last one.
    pub fn slot_offset(&self, slot: u32) -> Option<usize> {
        self.slot_at.get(usize::try_from(slot).ok()?).copied()
    }
}

/// Fills in the header of a fresh mapping.
///
/// # Safety
///
/// `base` points at [`HEADER_BYTES`] writable bytes, page-aligned, that no
/// other process can reach yet.
pub(crate) unsafe fn write_header(base: *mut u8, spec: &Spec) {
    // SAFETY: every offset is inside the header page and aligned to its
    // field's size, by the caller's contract and the constants above.
    unsafe {
        ptr::copy_nonoverlapping(MAGIC.as_ptr(), base.add(header::MAGIC), MAGIC.len());
        store_u32(base, header::VERSION, VERSION);
        store_u32(base, header::WIDTH, spec.width);
        store_u32(base, header::HEIGHT, spec.height);
        store_u32(base, header::STRIDE, spec.stride);
        store_u32(base, header::SLOTS, SLOTS);
        store_u64(base, header::SLOT_BYTES, spec.slot_bytes as u64);
    }
}

/// Whether a mapping's header describes exactly this spec.
///
/// # Safety
///
/// `base` points at a live mapping of at least [`HEADER_BYTES`], page
/// aligned.
#[cfg(any(test, feature = "writer"))]
pub(crate) unsafe fn header_matches(base: *const u8, spec: &Spec) -> bool {
    // SAFETY: as `write_header`. Another process may write the header as
    // this reads it, so the reads are volatile and the worst a race does
    // is a wrong answer here.
    unsafe {
        let magic = ptr::read_volatile(base.add(header::MAGIC).cast::<[u8; 4]>());

        magic == MAGIC
            && load_u32(base, header::VERSION) == VERSION
            && load_u32(base, header::WIDTH) == spec.width
            && load_u32(base, header::HEIGHT) == spec.height
            && load_u32(base, header::STRIDE) == spec.stride
            && load_u32(base, header::SLOTS) == SLOTS
            && load_u64(base, header::SLOT_BYTES) == spec.slot_bytes as u64
    }
}

// The mapping is shared with another process, so these go through raw
// pointers and volatile access. A `&u32` into it would tell the compiler
// the value can't change, and the renderer changes it whenever it likes.

/// # Safety
///
/// `base + at` is inside a live mapping and 4-aligned.
pub(crate) unsafe fn load_u32(base: *const u8, at: usize) -> u32 {
    // SAFETY: the caller's contract.
    u32::from_le(unsafe { ptr::read_volatile(base.add(at).cast::<u32>()) })
}

/// # Safety
///
/// `base + at` is inside a live mapping and 8-aligned.
pub(crate) unsafe fn load_u64(base: *const u8, at: usize) -> u64 {
    // SAFETY: the caller's contract.
    u64::from_le(unsafe { ptr::read_volatile(base.add(at).cast::<u64>()) })
}

/// # Safety
///
/// `base + at` is inside a live, writable mapping and 4-aligned.
pub(crate) unsafe fn store_u32(base: *mut u8, at: usize, value: u32) {
    // SAFETY: the caller's contract.
    unsafe { ptr::write_volatile(base.add(at).cast::<u32>(), value.to_le()) }
}

/// # Safety
///
/// `base + at` is inside a live, writable mapping and 8-aligned.
pub(crate) unsafe fn store_u64(base: *mut u8, at: usize, value: u64) {
    // SAFETY: the caller's contract.
    unsafe { ptr::write_volatile(base.add(at).cast::<u64>(), value.to_le()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_that_overflows_is_an_error() {
        // The stride is a u32 in the header, so a width past a quarter of
        // u32 overflows there first.
        assert!(matches!(Spec::exact(u32::MAX, 16), Err(Error::Overflow)));
        assert!(matches!(Spec::exact(1 << 30, 16), Err(Error::Overflow)));

        // A stride that fits and a frame that fits a u64, with two slots of
        // it that don't.
        assert!(matches!(
            Spec::exact((1 << 30) - 1, u32::MAX),
            Err(Error::Overflow)
        ));

        // Fits a u64 and passes the 4 GiB a 32-bit build can address.
        let past_32_bits = Spec::exact(65536, 65536);
        assert_eq!(past_32_bits.is_err(), usize::BITS < 64);
    }

    #[test]
    fn a_spec_clamps_to_the_contract_range() {
        let small = Spec::new(0, 1).unwrap();
        assert_eq!((small.width(), small.height()), (16, 16));

        let big = Spec::new(u32::MAX, u32::MAX).unwrap();
        assert_eq!((big.width(), big.height()), (3840, 2160));
        assert_eq!(big.pixel_bytes(), 3840 * 4 * 2160);
        assert_eq!(big.slot_bytes() % PAGE as usize, 0);
        assert_eq!(big.bytes(), HEADER_BYTES + 2 * big.slot_bytes());
        assert_eq!(big.slot_offset(1), Some(HEADER_BYTES + big.slot_bytes()));
        assert_eq!(big.slot_offset(2), None);
    }
}
