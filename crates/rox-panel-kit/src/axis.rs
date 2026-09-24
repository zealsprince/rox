//! Frequency labels for the surfaces that draw a Hz scale: a slider readout
//! with its unit, and a compact axis step without one.

pub fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else {
        format!("{:.0} Hz", hz.round())
    }
}

pub fn fmt_axis_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.0}k", hz / 1000.0)
    } else {
        format!("{hz:.0}")
    }
}
