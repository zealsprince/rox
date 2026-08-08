//! Shaping curves the visual surfaces share. Pure arithmetic, no state, so
//! anything that fades or ramps by distance reads the same math.

/// A step's weight under a compounding falloff: `dim` shaved off per step
/// away from the focus, so distance zero is full and each step further
/// multiplies by what's left. A zero factor never fades anything.
pub fn falloff(dim: f32, distance: u32) -> f32 {
    if dim <= 0.0 {
        return 1.0;
    }
    (1.0 - dim).powi(distance as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff_compounds_with_distance() {
        assert_eq!(falloff(0.0, 5), 1.0);
        assert_eq!(falloff(0.25, 0), 1.0);
        assert!((falloff(0.5, 1) - 0.5).abs() < f32::EPSILON);
        assert!((falloff(0.5, 3) - 0.125).abs() < f32::EPSILON);
        // A full shave leaves nothing past the focus itself.
        assert_eq!(falloff(1.0, 1), 0.0);
    }
}
