//! Frequency labels for the surfaces that draw a Hz scale: a slider readout
//! with its unit, and a compact axis step without one.

pub fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        rox_i18n::format::format_khz(f64::from(hz) / 1000.0, 1)
    } else {
        rox_i18n::format::format_hz(f64::from(hz.round()), 0)
    }
}

pub fn fmt_axis_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.0}k", hz / 1000.0)
    } else {
        format!("{hz:.0}")
    }
}
