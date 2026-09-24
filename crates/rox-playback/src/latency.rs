//! The low-latency hold (ADR 19): while a parameter editor is open, the decode
//! thread keeps the ring shallow so a knob is heard in [`LOW_LATENCY_MS`]
//! instead of the ring's full 500 ms. The ring is never resized; only how
//! full it's allowed to get changes.
//!
//! Process-global and refcounted, so any number of editor surfaces can hold it
//! and the decode thread pays one relaxed load per pass.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Ring depth while a hold is out. Above a 10-40 ms shared-mode period plus
/// the decode loop's 3 ms sleep; below the ~150 ms where a slider stops
/// feeling attached.
pub const LOW_LATENCY_MS: usize = 120;

static HOLDS: AtomicUsize = AtomicUsize::new(0);

/// Drop it to release.
pub struct LatencyHold {
    _private: (),
}

pub fn hold() -> LatencyHold {
    HOLDS.fetch_add(1, Ordering::Relaxed);
    LatencyHold { _private: () }
}

impl Drop for LatencyHold {
    fn drop(&mut self) {
        HOLDS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn held() -> bool {
    HOLDS.load(Ordering::Relaxed) > 0
}

/// The ring's allowed fill in interleaved samples. Full capacity with no hold
/// out, so the normal path costs one atomic load.
pub fn fill_limit(capacity: usize, device_rate: u32) -> usize {
    if !held() {
        return capacity;
    }
    let frames = device_rate as usize * LOW_LATENCY_MS / 1000;
    (frames * 2).min(capacity)
}

/// Samples the decode thread may push now. Zero while the ring holds more
/// than the target, so taking a hold drains the excess instead of dropping it.
pub fn push_room(capacity: usize, free: usize, device_rate: u32) -> usize {
    fill_limit(capacity, device_rate).saturating_sub(capacity - free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The refcount is process-global and tests run in parallel. Poison is ignored
    /// so one failure doesn't cascade.
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
