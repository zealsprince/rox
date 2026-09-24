//! The lane-packing geometry shared by the album grid, the genre wall, and
//! the artist wall. Pure numbers, testable without a window.

use gpui::{Along, Axis, Pixels, Point, px};

pub const TILE_DIM_MAX: f32 = 100.;

/// Two truncated lines plus a gap, fixed so the virtual list's item sizes
/// stay predictable.
pub const TILE_LABEL_H: f32 = 40.;

const PAGE_LINES: usize = 4;

pub fn default_dim() -> f32 {
    60.
}

pub fn default_gap() -> f32 {
    8.
}

#[derive(Clone, Copy, Debug)]
pub struct WallLayout {
    /// Extent across the packing axis; zero before the first paint.
    pub cross: Pixels,
    pub tile: f32,
    pub gap: f32,
    pub labels: bool,
    pub vertical: bool,
    /// Percent of fully hidden.
    pub dim: f32,
    pub dim_playing: bool,
    /// Keep the focus effects on while nothing plays.
    pub dim_always: bool,
    pub desaturate_playing: bool,
    pub hovered: Option<usize>,
    pub playing_ix: Option<usize>,
    pub playing: bool,
    /// Lanes before the first paint has measured `cross`.
    pub fallback_lanes: usize,
    pub label_h: Option<f32>,
}

impl WallLayout {
    /// Ceil so the actual edge never exceeds the configured one and nothing
    /// upscales past the stored thumbnail.
    pub fn lanes(&self) -> usize {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return self.fallback_lanes;
        }
        let gap = self.gap;
        // A horizontal wall stacks captions across its lanes; a vertical one
        // sends them into the scroll.
        let footprint = self.tile + self.cross_label();
        (((cross + gap) / (footprint + gap)).ceil() as usize).max(1)
    }

    pub fn label_height(&self) -> f32 {
        if self.labels {
            self.label_h.unwrap_or(TILE_LABEL_H)
        } else {
            0.
        }
    }

    pub fn cross_label(&self) -> f32 {
        if self.vertical {
            0.
        } else {
            self.label_height()
        }
    }

    pub fn axis(&self) -> Axis {
        if self.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The leading tile in view, for the saved layout. A restore still pending
    /// reports its own target so an unshown panel keeps its position. `offset`
    /// runs negative as the list scrolls.
    pub fn first_cell(&self, restore: Option<usize>, offset: Point<Pixels>, cells: usize) -> usize {
        if let Some(cell) = restore {
            return cell;
        }
        let lanes = self.lanes();
        // Must match the item sizes the virtual list lays out, or a restored
        // cell drifts. Only a vertical wall's captions add to the pitch.
        let scroll_label = if self.vertical {
            self.label_height()
        } else {
            0.
        };
        let extent = f32::from(self.tile_side()) + scroll_label + self.gap;
        if extent <= 0. {
            return 0;
        }
        let offset = f32::from(-offset.along(self.axis()));
        let line = (offset / extent).floor().max(0.) as usize;
        (line * lanes).min(cells.saturating_sub(1))
    }

    pub fn tile_side(&self) -> Pixels {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return px(self.tile);
        }
        let lanes = self.lanes() as f32;
        px((((cross - self.gap * (lanes - 1.)) / lanes) - self.cross_label()).max(1.))
    }

    /// The hovered and playing tiles are always exempt; the rest recede while
    /// audio moves, or always in always mode.
    pub fn receded(&self, ix: usize) -> bool {
        if self.hovered == Some(ix) || self.playing_ix == Some(ix) {
            return false;
        }
        self.dim_always || self.playing
    }

    pub fn dim_target(&self, ix: usize) -> f32 {
        if self.dim_playing && self.receded(ix) {
            1.0 - self.dim / TILE_DIM_MAX
        } else {
            1.0
        }
    }

    pub fn desaturated(&self, ix: usize) -> bool {
        self.desaturate_playing && self.receded(ix)
    }

    pub fn page_step(&self) -> isize {
        (PAGE_LINES * self.lanes()) as isize
    }

    /// One tile along a line, a whole line across. Which arrows are which
    /// flips with the wall's orientation.
    pub fn step(&self, key: &str) -> Option<isize> {
        let line = self.lanes() as isize;
        let (along, across) = if self.vertical {
            (("left", "right"), ("up", "down"))
        } else {
            (("up", "down"), ("left", "right"))
        };
        match key {
            k if k == along.0 => Some(-1),
            k if k == along.1 => Some(1),
            k if k == across.0 => Some(-line),
            k if k == across.1 => Some(line),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> WallLayout {
        WallLayout {
            cross: px(0.),
            tile: 160.,
            gap: 8.,
            labels: false,
            vertical: true,
            dim: 60.,
            dim_playing: false,
            dim_always: false,
            desaturate_playing: false,
            hovered: None,
            playing_ix: None,
            playing: false,
            fallback_lanes: 5,
            label_h: None,
        }
    }

    #[test]
    fn arrows_step_along_and_across_the_lines() {
        let layout = wall();
        assert_eq!(layout.step("right"), Some(1));
        assert_eq!(layout.step("left"), Some(-1));
        assert_eq!(layout.step("down"), Some(5), "a whole line down");
        assert_eq!(layout.step("up"), Some(-5));
        assert_eq!(layout.step("enter"), None);
    }

    #[test]
    fn a_horizontal_wall_swaps_the_arrow_pair() {
        let layout = WallLayout {
            vertical: false,
            ..wall()
        };
        assert_eq!(layout.step("down"), Some(1));
        assert_eq!(layout.step("up"), Some(-1));
        assert_eq!(layout.step("right"), Some(5));
        assert_eq!(layout.step("left"), Some(-5));
    }

    #[test]
    fn unmeasured_wall_uses_its_fallback_lanes() {
        let layout = wall();
        assert_eq!(layout.lanes(), 5);
        assert_eq!(layout.tile_side(), px(160.));
    }

    #[test]
    fn lanes_never_upscale_past_the_configured_edge() {
        let layout = WallLayout {
            cross: px(600.),
            ..wall()
        };
        let lanes = layout.lanes();
        assert_eq!(lanes, 4);
        assert!(layout.tile_side() <= px(160.));
        let side = f32::from(layout.tile_side());
        assert!((side * lanes as f32 + 8. * (lanes as f32 - 1.) - 600.).abs() < 0.01);
    }

    #[test]
    fn captions_take_a_lane_only_on_a_horizontal_wall() {
        let bare = WallLayout {
            cross: px(600.),
            ..wall()
        };
        let vertical = WallLayout {
            labels: true,
            ..bare
        };
        let horizontal = WallLayout {
            vertical: false,
            ..vertical
        };
        assert_eq!(vertical.cross_label(), 0.);
        assert_eq!(horizontal.cross_label(), TILE_LABEL_H);
        assert_eq!(vertical.lanes(), bare.lanes());
        assert!(horizontal.lanes() < vertical.lanes());
        assert_eq!(vertical.axis(), Axis::Vertical);
        assert_eq!(horizontal.axis(), Axis::Horizontal);
    }

    #[test]
    fn a_pending_restore_reports_its_own_target() {
        let layout = WallLayout {
            cross: px(600.),
            ..wall()
        };
        assert_eq!(layout.first_cell(Some(42), Point::default(), 100), 42);
    }

    #[test]
    fn first_cell_spreads_the_leading_line_over_the_lanes() {
        let layout = WallLayout {
            cross: px(600.),
            ..wall()
        };
        let pitch = f32::from(layout.tile_side()) + layout.gap;
        let offset = Point {
            x: px(0.),
            y: px(-pitch * 3.),
        };
        assert_eq!(layout.first_cell(None, offset, 100), layout.lanes() * 3);
        let far = Point {
            x: px(0.),
            y: px(-pitch * 900.),
        };
        assert_eq!(layout.first_cell(None, far, 10), 9);
    }

    #[test]
    fn hovered_and_playing_tiles_stay_out_of_the_receded_set() {
        let layout = WallLayout {
            dim_playing: true,
            desaturate_playing: true,
            playing: true,
            hovered: Some(1),
            playing_ix: Some(2),
            ..wall()
        };
        assert!(!layout.receded(1));
        assert!(!layout.receded(2));
        assert!(layout.receded(3));
        assert_eq!(layout.dim_target(1), 1.0);
        assert!((layout.dim_target(3) - 0.4).abs() < f32::EPSILON);
        assert!(!layout.desaturated(2));
        assert!(layout.desaturated(3));
    }

    #[test]
    fn always_mode_recedes_without_a_track_playing() {
        let layout = WallLayout {
            dim_playing: true,
            dim_always: true,
            ..wall()
        };
        assert!(layout.receded(0));
        assert!((layout.dim_target(0) - 0.4).abs() < f32::EPSILON);
    }
}
