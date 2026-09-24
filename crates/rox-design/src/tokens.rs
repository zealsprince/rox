//! The app's non-color tokens per ADR 12. Layout tokens are [`Pixels`];
//! paint tokens are plain `f32` because canvas closures do their math in f32.
//! A value one control uses in one place stays a local const there.

use gpui::{Pixels, px};

// Motion.

/// The one pace every transition shares, so nothing drifts out of step.
pub const EASE_SECS: f32 = 0.35;

// Radii. Fully-round shapes stay `rounded_full`.

pub const RADIUS: Pixels = px(6.);

// The spacing ladder.

pub const SPACE_XS: Pixels = px(4.);
pub const SPACE_SM: Pixels = px(8.);
pub const SPACE_MD: Pixels = px(12.);

// Audio controls, layout side.

/// Padding around the icon button's 16px glyph.
pub const ICON_PAD: Pixels = px(6.);
pub const PLAY_SIZE: Pixels = px(30.);
pub const CONTROL_H: Pixels = px(22.);
pub const SLIDER_MIN_W: Pixels = px(80.);
pub const SLIDER_MAX_W: Pixels = px(200.);

// Audio controls, paint side.

pub const SLIDER_TRACK_H: f32 = 4.0;
pub const SLIDER_KNOB: f32 = 12.0;
pub const SEEK_STRIP_H: f32 = 6.0;
pub const PLAYHEAD_W: f32 = 2.0;
/// The bar rhythm the waveform and spectrum share.
pub const BAR_W: f32 = 3.0;
pub const BAR_GAP: f32 = 2.0;
