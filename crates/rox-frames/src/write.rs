//! The renderer's side of the mapping, for tests and the example
//! renderer. It's behind the `writer` feature, which rox's own build never
//! turns on. A writer trusts rox no more than rox trusts it: it checks the
//! file's size and header against the spec it was told before it writes.

use std::sync::atomic::{Ordering, fence};

use crate::layout::{self, slot};
use crate::map::{Handle, os};
use crate::{Error, Spec, clock_ns};

/// A read-write view of a frame mapping, writing frames the way the
/// contract says a renderer does.
pub struct Writer {
    view: os::View,
    spec: Spec,
}

// SAFETY: the view is a raw region written only through `&mut self`.
unsafe impl Send for Writer {}

impl Writer {
    /// Maps `handle` read-write and checks it describes exactly `spec`.
    ///
    /// # Safety
    ///
    /// No other writer, in this process or another, maps the same file for
    /// the writer's life. [`Writer::frame`] hands out `&mut` to the pixels.
    pub unsafe fn open(handle: Handle<'_>, spec: Spec) -> Result<Writer, Error> {
        // SAFETY: the caller's contract.
        let view = unsafe { os::map_writable(handle, &spec)? };

        // SAFETY: the view is `spec.bytes()` long, which is more than the
        // header, and page-aligned.
        if !unsafe { layout::header_matches(view.base(), &spec) } {
            return Err(Error::Mismatch);
        }

        Ok(Writer { view, spec })
    }

    /// Writes frame `n` into slot `n % 2`: the counter goes odd, `draw`
    /// fills the pixels, the stamp goes in, and the counter goes even.
    /// Returns the slot, for the announce line.
    pub fn frame(&mut self, n: u64, draw: impl FnOnce(&mut [u8])) -> u32 {
        let (width, height) = (self.spec.width(), self.spec.height());
        self.frame_claiming(n, width, height, draw)
    }

    /// Writes frame `n` drawn at `width` by `height`, which a renderer told
    /// a new size does: `draw` gets `width * height * 4` bytes from the
    /// slot's start, rows packed, and the slot says that size. A size past
    /// the mapping is held to it.
    pub fn frame_at(
        &mut self,
        n: u64,
        width: u32,
        height: u32,
        draw: impl FnOnce(&mut [u8]),
    ) -> u32 {
        let width = width.clamp(1, self.spec.width());
        let height = height.clamp(1, self.spec.height());
        let len = width as usize * height as usize * layout::PIXEL as usize;

        self.frame_claiming(n, width, height, |pixels| draw(&mut pixels[..len]))
    }

    /// `frame`, with whatever size the slot should claim.
    pub(crate) fn frame_claiming(
        &mut self,
        n: u64,
        width: u32,
        height: u32,
        draw: impl FnOnce(&mut [u8]),
    ) -> u32 {
        let slot = u32::from(n % 2 == 1);
        let at = self.spec.slot_offset(slot).unwrap_or(layout::HEADER_BYTES);
        let len = self.spec.pixel_bytes();
        let base = self.view.base_mut();

        // Odd and even by construction, whatever `n` is.
        let writing = n.wrapping_mul(2).wrapping_add(1);
        let whole = writing.wrapping_add(1);

        // SAFETY: `at` is a slot start from the spec, inside the view and
        // 4096-aligned, so the slot header's fields are in bounds and
        // aligned.
        unsafe {
            layout::store_u64(base, at + slot::SEQ, writing);
        }

        // Keeps the odd counter ahead of every write to the slot.
        fence(Ordering::Release);

        // SAFETY: as above. The pixels run `len` bytes from the slot's
        // pixel offset, inside the slot by the spec's arithmetic. `open`'s
        // contract makes this the only writer, so the `&mut` is unique in
        // this process. rox reads the same bytes from its own process,
        // which this process can't observe and which can't invalidate the
        // borrow.
        unsafe {
            layout::store_u32(base, at + slot::WIDTH, width);
            layout::store_u32(base, at + slot::HEIGHT, height);
            draw(std::slice::from_raw_parts_mut(
                base.add(at + slot::PIXELS),
                len,
            ));
            layout::store_u64(base, at + slot::STAMP_NS, clock_ns());
        }

        // Keeps every write to the slot ahead of the even counter.
        fence(Ordering::Release);

        // SAFETY: as the first counter store.
        unsafe {
            layout::store_u64(base, at + slot::SEQ, whole);
        }

        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mapping;

    #[test]
    fn a_writer_told_another_size_refuses() {
        let mapping = Mapping::create(Spec::new(32, 32).unwrap()).unwrap();

        for other in [Spec::new(16, 32).unwrap(), Spec::new(32, 48).unwrap()] {
            // SAFETY: nothing maps the file for writing, and the open fails.
            let opened = unsafe { Writer::open(mapping.child_handle(), other) };
            assert!(matches!(opened, Err(Error::Mismatch)));
        }
    }
}
