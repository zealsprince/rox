//! The serde helpers every panel config reaches for. Small enough that each
//! panel used to carry its own copy, which is exactly why they belong in one
//! place.

/// A knob that ships on, so an older layout dump missing the field keeps the
/// behaviour it had.
pub fn default_true() -> bool {
    true
}

/// Whether a counter is at rest, for the `skip_serializing_if` on the
/// saved-position fields: a wall parked at the top writes nothing.
pub fn is_zero(n: &usize) -> bool {
    *n == 0
}
