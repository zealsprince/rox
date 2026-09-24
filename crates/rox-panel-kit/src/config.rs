//! The serde helpers every panel config uses, kept in one place so no panel
//! carries its own copy.

/// Serde default for a knob that ships on, so an older dump keeps it on.
pub fn default_true() -> bool {
    true
}

pub fn is_zero(n: &usize) -> bool {
    *n == 0
}
