//! Frequency labels for the surfaces that draw a Hz scale: the spectrum
//! panel's band sliders and the equalizer window's ladder. Two widths, one
//! for a readout that has room for the unit and one for a ladder step that
//! doesn't.

/// A bound's Hz for a slider readout, compact enough for the strip.
pub fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz.round())
    }
}

/// A ladder step's label for an axis, where a dozen of them share the width
/// and the unit is the one thing the reader already knows.
pub fn fmt_axis_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.0}k", hz / 1000.0)
    } else {
        format!("{hz:.0}")
    }
}
