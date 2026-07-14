//! The app's non-color tokens per ADR 12: every size, radius, and pace
//! the panels share, one const per decision, beside the palette so both
//! read the same at a call site. Layout tokens are [`Pixels`] and feed
//! div chains directly; paint tokens are plain `f32` because canvas
//! closures do their math in f32 before wrapping in `px()`. A value that
//! belongs to one control in one place stays a local const there - a
//! token earns its slot when two files must agree or a look-wide knob
//! should turn in one line.

use gpui::{px, Pixels};

// Motion.

/// The one pace every transition shares: palette changes, the cover
/// fade, the backdrop crossfade, the waveform reveal. One knob so
/// nothing drifts out of step.
pub const EASE_SECS: f32 = 0.35;

// Radii. Fully-round shapes (the play button, the toggle) stay
// `rounded_full`: full round is intent, not a value.

/// The corner radius on every squared control: icon buttons, inputs,
/// segmented choices, dialog buttons.
pub const RADIUS: Pixels = px(6.);

// The spacing ladder: gaps and insets pull from these three steps.

/// Tight spacing: inside a control cluster, a row's vertical inset.
pub const SPACE_XS: Pixels = px(4.);
/// The default spacing between items in a row and a row's horizontal inset.
pub const SPACE_SM: Pixels = px(8.);
/// Roomy spacing: between setting rows, a window body's inset.
pub const SPACE_MD: Pixels = px(12.);

// Audio controls, layout side.

/// The icon button's padding around its 16px glyph.
pub const ICON_PAD: Pixels = px(6.);
/// The filled round play/pause button.
pub const PLAY_SIZE: Pixels = px(30.);
/// The compact control height: toolbar buttons, the search box, a
/// slider strip's hit band.
pub const CONTROL_H: Pixels = px(22.);
/// The width band a slider claims: never squeezed under the floor,
/// capped at its natural size unless the panel stretches it.
pub const SLIDER_MIN_W: Pixels = px(80.);
pub const SLIDER_MAX_W: Pixels = px(200.);

// Audio controls, paint side.

/// A slider's track line and the round knob riding it.
pub const SLIDER_TRACK_H: f32 = 4.0;
pub const SLIDER_KNOB: f32 = 12.0;
/// The seek strip's track line, thicker than a slider's: it is the
/// whole control.
pub const SEEK_STRIP_H: f32 = 6.0;
/// The playhead line on the seek strip and the waveform.
pub const PLAYHEAD_W: f32 = 2.0;
/// The visualizer bar rhythm the waveform and spectrum agree on: bars
/// never thinner than this, this much air between them.
pub const BAR_W: f32 = 3.0;
pub const BAR_GAP: f32 = 2.0;
