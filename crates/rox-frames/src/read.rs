//! Copying a frame out of the mapping. The renderer may be writing the
//! slot while rox reads it, and nothing locks across the boundary: the
//! slot's counter, read before and after the copy, is what says whether
//! the copy is whole.

use std::ptr;
use std::sync::atomic::{Ordering, fence};

use crate::layout::{SLOTS, load_u32, load_u64, slot};
use crate::map::Mapping;

/// What came of copying a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// The copy is one whole frame.
    Whole {
        /// The slot's counter, even, as it stood across the copy.
        seq: u64,
        /// The renderer's stamp, for latency stats only.
        stamp_ns: u64,
        /// The size the frame was drawn at, as the slot said and the caller
        /// accepted. The copy holds `width * height * 4` bytes, rows packed.
        width: u32,
        height: u32,
    },
    /// The renderer wrote the slot during the copy.
    Torn,
    /// The slot can't be copied at all.
    Refused(Refusal),
}

/// Why a slot was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The announce named a slot past the last.
    NoSuchSlot(u32),
    /// Nothing has been written to the slot yet.
    Empty,
    /// The counter is odd: the renderer is writing.
    Writing,
    /// The slot says its frame is a size rox didn't ask for, or one that
    /// doesn't fit the mapping.
    Size { width: u32, height: u32 },
}

impl Mapping {
    /// Copies a slot's pixels into `into`, which ends up exactly
    /// `spec().pixel_bytes()` long. Its contents mean something only when
    /// this returns [`Read::Whole`].
    pub fn copy_slot(&self, slot: u32, into: &mut Vec<u8>) -> Read {
        let (width, height) = (self.spec().width(), self.spec().height());
        self.copy_slot_sized(slot, |w, h| (w, h) == (width, height), into)
    }

    /// Copies a frame drawn at a size `accept` takes, which can be smaller
    /// than the mapping: a renderer told a new size draws into the slot's
    /// first `width * height * 4` bytes, rows packed. `into` ends up that
    /// long. A size that doesn't fit the mapping is refused whatever
    /// `accept` says, so every length still comes from rox's own spec.
    pub fn copy_slot_sized(
        &self,
        slot: u32,
        accept: impl Fn(u32, u32) -> bool,
        into: &mut Vec<u8>,
    ) -> Read {
        self.copy_slot_between(slot, accept, into, || {})
    }

    /// `copy_slot_sized` with a hook between the copy and the second
    /// counter read, so a test can move the counter there.
    pub(crate) fn copy_slot_between(
        &self,
        slot: u32,
        accept: impl Fn(u32, u32) -> bool,
        into: &mut Vec<u8>,
        between: impl FnOnce(),
    ) -> Read {
        let spec = self.spec();
        if slot >= SLOTS {
            return Read::Refused(Refusal::NoSuchSlot(slot));
        }
        let Some(at) = spec.slot_offset(slot) else {
            return Read::Refused(Refusal::NoSuchSlot(slot));
        };

        let base = self.base();

        // SAFETY: `at` is a slot start from the spec, so it's inside the
        // view, 4096-aligned, and followed by the slot's 64 byte header.
        // The read is volatile because the renderer may change the counter
        // at any moment; a stale value at worst fails the check below.
        let before = unsafe { load_u64(base, at + slot::SEQ) };
        if before == 0 {
            return Read::Refused(Refusal::Empty);
        }
        if before & 1 == 1 {
            return Read::Refused(Refusal::Writing);
        }

        // Keeps the size words and the copy after the first counter read,
        // on weakly ordered CPUs too.
        fence(Ordering::Acquire);

        // SAFETY: as the counter, inside the slot header and 4-aligned.
        let (width, height) = unsafe {
            (
                load_u32(base, at + slot::WIDTH),
                load_u32(base, at + slot::HEIGHT),
            )
        };
        let fits = (1..=spec.width()).contains(&width) && (1..=spec.height()).contains(&height);
        if !fits || !accept(width, height) {
            return Read::Refused(Refusal::Size { width, height });
        }

        // Both sides are at most the spec's, so this is at most its
        // `pixel_bytes`, which the spec computed with checked arithmetic.
        let len = width as usize * height as usize * crate::layout::PIXEL as usize;
        into.resize(len, 0);

        // SAFETY: the source runs from the slot's pixels for `len` bytes,
        // at most the spec's frame, which its checked arithmetic puts inside
        // this slot of the view, and `into` was just sized to `len`. Both lengths are rox's;
        // the slot's own size words only decide whether to copy at all.
        // The renderer may write the source as this reads it. That's a
        // data race in Rust's model, but it's confined to bytes copied
        // through a raw pointer and never interpreted. A torn copy is at
        // worst wrong pixels, and the counter check below throws it away.
        unsafe {
            ptr::copy_nonoverlapping(base.add(at + slot::PIXELS), into.as_mut_ptr(), len);
        }

        between();

        // SAFETY: as the counter.
        let stamp_ns = unsafe { load_u64(base, at + slot::STAMP_NS) };

        // Keeps the copy and the stamp before the second counter read.
        fence(Ordering::Acquire);

        // SAFETY: as the counter.
        let after = unsafe { load_u64(base, at + slot::SEQ) };
        if after != before {
            return Read::Torn;
        }

        Read::Whole {
            seq: before,
            stamp_ns,
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Spec, Writer};

    fn session(width: u32, height: u32) -> (Mapping, Writer) {
        let spec = Spec::new(width, height).unwrap();
        let mapping = Mapping::create(spec).unwrap();

        // SAFETY: the only writer for this mapping in the test.
        let writer = unsafe { Writer::open(mapping.child_handle(), spec) }.unwrap();

        (mapping, writer)
    }

    fn pattern(n: u64) -> impl FnOnce(&mut [u8]) {
        move |pixels| {
            for (i, byte) in pixels.iter_mut().enumerate() {
                *byte = (i as u64).wrapping_add(n) as u8;
            }
        }
    }

    #[test]
    fn a_frame_written_is_the_frame_read() {
        let (mapping, mut writer) = session(32, 16);
        let mut frame = Vec::new();

        for n in [0, 1, 2, 7] {
            let slot = writer.frame(n, pattern(n));
            assert_eq!(slot, (n % 2) as u32);

            let read = mapping.copy_slot(slot, &mut frame);
            let Read::Whole { seq, stamp_ns, .. } = read else {
                panic!("frame {n}: {read:?}");
            };
            assert_eq!(seq % 2, 0);
            assert!(stamp_ns > 0);

            let mut want = vec![0; mapping.spec().pixel_bytes()];
            pattern(n)(&mut want);
            assert_eq!(frame, want);
        }
    }

    #[test]
    fn an_odd_counter_is_refused() {
        let (mapping, mut writer) = session(16, 16);
        let mut frame = Vec::new();

        // Read from inside the write, while the counter is odd.
        let mut seen = None;
        writer.frame(4, |_| seen = Some(mapping.copy_slot(0, &mut frame)));

        assert_eq!(seen, Some(Read::Refused(Refusal::Writing)));
        assert_eq!(
            mapping.copy_slot(1, &mut frame),
            Read::Refused(Refusal::Empty)
        );
    }

    #[test]
    fn a_frame_that_moves_under_the_copy_is_torn() {
        let (mapping, mut writer) = session(16, 16);
        let mut frame = Vec::new();

        writer.frame(0, pattern(0));
        let read = mapping.copy_slot_between(
            0,
            |_, _| true,
            &mut frame,
            || {
                writer.frame(2, pattern(2));
            },
        );

        assert_eq!(read, Read::Torn);
        assert!(matches!(
            mapping.copy_slot(0, &mut frame),
            Read::Whole { .. }
        ));
    }

    #[test]
    fn a_slot_claiming_another_size_is_refused() {
        let (mapping, mut writer) = session(16, 16);
        let mut frame = Vec::new();

        let slot = writer.frame_claiming(0, 32, 16, |_| {});

        assert_eq!(
            mapping.copy_slot(slot, &mut frame),
            Read::Refused(Refusal::Size {
                width: 32,
                height: 16
            })
        );
    }

    #[test]
    fn a_frame_drawn_smaller_is_read_at_its_own_size() {
        let (mapping, mut writer) = session(64, 32);
        let mut frame = Vec::new();

        let slot = writer.frame_at(0, 24, 16, pattern(0));
        let read = mapping.copy_slot_sized(slot, |w, h| (w, h) == (24, 16), &mut frame);
        assert!(
            matches!(
                read,
                Read::Whole {
                    width: 24,
                    height: 16,
                    ..
                }
            ),
            "{read:?}"
        );

        // Packed at its own width, and nothing past it.
        let mut want = vec![0; 24 * 16 * 4];
        pattern(0)(&mut want);
        assert_eq!(frame, want);

        // A size the caller didn't tell it is refused, and so is one past
        // the mapping, whatever the caller would take.
        assert_eq!(
            mapping.copy_slot(slot, &mut frame),
            Read::Refused(Refusal::Size {
                width: 24,
                height: 16
            })
        );
        let slot = writer.frame_claiming(1, 65, 16, |_| {});
        assert_eq!(
            mapping.copy_slot_sized(slot, |_, _| true, &mut frame),
            Read::Refused(Refusal::Size {
                width: 65,
                height: 16
            })
        );
        let slot = writer.frame_claiming(2, 0, 16, |_| {});
        assert!(matches!(
            mapping.copy_slot_sized(slot, |_, _| true, &mut frame),
            Read::Refused(Refusal::Size { .. })
        ));
    }

    #[test]
    fn a_slot_past_the_second_is_refused() {
        let (mapping, mut writer) = session(16, 16);
        let mut frame = Vec::new();
        writer.frame(0, pattern(0));

        for slot in [2, 3, u32::MAX] {
            assert_eq!(
                mapping.copy_slot(slot, &mut frame),
                Read::Refused(Refusal::NoSuchSlot(slot))
            );
        }
    }
}
