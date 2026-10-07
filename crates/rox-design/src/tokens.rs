//! The app's non-color tokens per ADR 12. Layout tokens are [`Pixels`];
//! paint tokens are plain `f32` because canvas closures do their math in f32.
//! A value one control uses in one place stays a local const there. The
//! slider's shape is the one token a workspace and a panel can override, so
//! it's live.

use std::sync::RwLock;

use gpui::{App, Pixels, px};
use serde::{Deserialize, Serialize};

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
/// Past this the knob is already a circle, so more changes nothing.
pub const SLIDER_ROUNDING_MAX: f32 = SLIDER_KNOB / 2.0;
pub const SEEK_STRIP_H: f32 = 6.0;
pub const PLAYHEAD_W: f32 = 2.0;
/// The bar rhythm the waveform and spectrum share.
pub const BAR_W: f32 = 3.0;
pub const BAR_GAP: f32 = 2.0;

/// How every value slider draws: the volume strips and the settings sliders.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SliderLook {
    /// Corner radius in px, capped at a pill for the track and the knob each.
    pub rounding: f32,
    /// Whether the drag knob draws. Off, the fill alone shows the level.
    pub knob: bool,
}

impl SliderLook {
    pub const DEFAULT: SliderLook = SliderLook {
        rounding: SLIDER_ROUNDING_MAX,
        knob: true,
    };
}

impl Default for SliderLook {
    fn default() -> Self {
        SliderLook::DEFAULT
    }
}

/// A static so paint closures read it without a context. The setter repaints,
/// since it sits outside gpui's reactivity.
static SLIDER: RwLock<SliderLook> = RwLock::new(SliderLook::DEFAULT);

/// The workspace's look, before any panel override.
pub fn app_slider_look() -> SliderLook {
    *SLIDER.read().unwrap()
}

/// The app look under the innermost panel's override. Paint closures can
/// run outside the scope, so callers read this while they build.
pub fn slider_look() -> SliderLook {
    let app = app_slider_look();
    let (rounding, knob) = crate::palette::scope_slider();

    SliderLook {
        rounding: rounding.map_or(app.rounding, |r| r.max(0.0)),
        knob: knob.unwrap_or(app.knob),
    }
}

pub fn set_slider_look(look: SliderLook, cx: &mut App) {
    let rounding = if look.rounding.is_finite() {
        look.rounding.max(0.0)
    } else {
        SLIDER_ROUNDING_MAX
    };
    *SLIDER.write().unwrap() = SliderLook { rounding, ..look };
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}
