//! The low-latency hold (ADR 19): while a parameter editor is open, the
//! decode thread keeps the sample ring shallow so a knob lands sooner.
//!
//! The chain runs pre-ring, so a parameter change is only audible once the
//! samples ahead of it drain, up to the ring's full 500 ms. The ring itself
//! is allocated once at stream open and never resized; what moves is how
//! full the decode thread lets it get. Hold this and the fill gates at
//! [`LOW_LATENCY_MS`], so the wait between slider and ear is that instead of
//! the whole cushion.
//!
//! Process-global, like the EQ's parameter atomics: the editor windows are
//! global too (one curve for every workspace), and the decode thread wants
//! one relaxed load per pass, not a route through the command channel.
//! Refcounted rather than a flag so a second editor surface can hold it
//! alongside the first without either one's close cutting the other short.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How much audio the ring may hold while a hold is out, in milliseconds.
/// The floor is the device: a shared-mode period runs 10-40 ms on a typical
/// desktop (PipeWire's 1024-frame quantum is 21 ms at 48 kHz), and the
/// decode loop naps 3 ms between refills, so 120 ms is still several periods
/// of cushion against a scheduling hiccup. The ceiling is the ear: past
/// roughly 150 ms a slider stops feeling attached to what it's moving.
pub const LOW_LATENCY_MS: usize = 120;

/// How many holds are out. Zero means the ring fills to the brim as always.
static HOLDS: AtomicUsize = AtomicUsize::new(0);

/// A live request for low parameter latency. Drop it to release; the decode
/// thread refills to full depth again on its next pass.
pub struct LatencyHold {
    _private: (),
}

/// Ask the decode thread to keep the ring shallow until the returned guard
/// drops. Cheap enough to take on a window open and forget about.
pub fn hold() -> LatencyHold {
    HOLDS.fetch_add(1, Ordering::Relaxed);
    LatencyHold { _private: () }
}

impl Drop for LatencyHold {
    fn drop(&mut self) {
        HOLDS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Whether anything is asking for low latency right now.
pub fn held() -> bool {
    HOLDS.load(Ordering::Relaxed) > 0
}

/// How full the ring is allowed to get, in interleaved stereo samples, given
/// its capacity and the device rate. The full capacity with no hold out, so
/// the gate costs one atomic load on the normal path; the target otherwise,
/// clamped to capacity in case a device ever opens with a ring shorter than
/// the target.
pub fn fill_limit(capacity: usize, device_rate: u32) -> usize {
    if !held() {
        return capacity;
    }
    let frames = device_rate as usize * LOW_LATENCY_MS / 1000;
    (frames * 2).min(capacity)
}

/// How many samples the decode thread may push right now, from the ring's
/// capacity and its free slots. Every free slot with no hold out, the room
/// left under the target otherwise, and zero once the ring already holds
/// more than the target, which is what makes taking a hold mid-playback
/// drain the excess instead of dropping it.
pub fn push_room(capacity: usize, free: usize, device_rate: u32) -> usize {
    fill_limit(capacity, device_rate).saturating_sub(capacity - free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The refcount is process-global and the harness runs tests on parallel
    /// threads, so without this one test's guards would answer another's
    /// `held`. Poison is ignored on purpose: a failed assertion in one
    /// shouldn't turn the rest into unrelated failures.
    static HOLDS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn holds_refcount_rather_than_flag() {
        let _serial = HOLDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!held(), "nothing held to start");
        let first = hold();
        let second = hold();
        assert!(held());
        // One editor closing while another is still open must not release.
        drop(first);
        assert!(held(), "the second hold still stands");
        drop(second);
        assert!(!held(), "the last drop releases");
    }

    #[test]
    fn fill_limit_gates_only_while_held() {
        let _serial = HOLDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 500 ms of stereo at 48 kHz, the real ring.
        let capacity = 48_000 / 2 * 2;
        assert_eq!(fill_limit(capacity, 48_000), capacity);
        let editor = hold();
        assert_eq!(fill_limit(capacity, 48_000), 48_000 * 120 / 1000 * 2);
        // A ring shorter than the target can't be filled past its end.
        assert_eq!(fill_limit(64, 48_000), 64);
        drop(editor);
        assert_eq!(fill_limit(capacity, 48_000), capacity);
    }

    #[test]
    fn push_room_is_every_free_slot_until_a_hold_lands() {
        let _serial = HOLDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let capacity = 48_000 / 2 * 2;
        // Unheld: the whole ring, half a ring, none of it.
        assert_eq!(push_room(capacity, capacity, 48_000), capacity);
        assert_eq!(push_room(capacity, capacity / 2, 48_000), capacity / 2);
        assert_eq!(push_room(capacity, 0, 48_000), 0);

        let editor = hold();
        let target = 48_000 * 120 / 1000 * 2;
        // An empty ring fills to the target and stops there.
        assert_eq!(push_room(capacity, capacity, 48_000), target);
        // Half the target in already, half the target left to push.
        assert_eq!(
            push_room(capacity, capacity - target / 2, 48_000),
            target / 2
        );
        // Deeper than the target: nothing goes in until it drains under.
        assert_eq!(push_room(capacity, capacity - target * 2, 48_000), 0);
        drop(editor);
    }
}
