//! The geometry behind the wall panels: the album grid, the genre wall, and
//! the artist wall all pack square tiles into lanes the same way, so the
//! packing math is here once and each panel just hands over its config.
//!
//! It computes in gpui [`Pixels`] against the panel's measured cross
//! extent, so it belongs in the widget layer rather than down in rox-viz.

use gpui::{px, Along, Axis, Pixels, Point};

/// The dim knob's ceiling, in percent of fully hidden: 100 fades the other
/// tiles out entirely.
pub const TILE_DIM_MAX: f32 = 100.;

/// The caption block's height under a tile while labels are on, in px: two
/// truncated text lines plus a little top gap. Fixed so the tile's total
/// extent stays predictable for the virtual list's item sizes.
pub const TILE_LABEL_H: f32 = 40.;

/// How many lines page up and page down cover. A wall shows a handful of
/// lines at a usable tile size, so a page is a small number of them rather
/// than the list panels' 25 rows.
const PAGE_LINES: usize = 4;

/// How far the dimmed tiles fade by default, in percent of fully hidden.
pub fn default_dim() -> f32 {
    60.
}

/// The default space between tiles, in px.
pub fn default_gap() -> f32 {
    8.
}

/// One wall's packing and focus state for the frame being laid out: the
/// tile knobs off the panel's config, the measured cross extent, and which
/// tile the pointer and the player are on.
///
/// A panel builds one of these per call. Every field is a plain number, so
/// it's cheap to rebuild and the math stays testable without a window.
#[derive(Clone, Copy, Debug)]
pub struct WallLayout {
    /// The panel's measured extent across the packing axis: the width of a
    /// vertical wall, the height of a horizontal one. Zero before the first
    /// paint has measured anything.
    pub cross: Pixels,
    /// The preferred tile edge in px, what the size knob sets.
    pub tile: f32,
    /// The space between tiles, in px.
    pub gap: f32,
    /// Whether captions show under every tile.
    pub labels: bool,
    /// Scroll the wall vertically, lines filling the width; off scrolls it
    /// horizontally, lines filling the height.
    pub vertical: bool,
    /// How far the receded tiles fade, in percent of fully hidden.
    pub dim: f32,
    /// Fade the tiles the focus effects push back.
    pub dim_playing: bool,
    /// Keep the focus effects on all the time, not only while a track
    /// plays.
    pub dim_always: bool,
    /// Drain the receded tiles to grayscale.
    pub desaturate_playing: bool,
    /// The tile under the pointer, exempt from the focus effects.
    pub hovered: Option<usize>,
    /// The tile the player is on, also exempt.
    pub playing_ix: Option<usize>,
    /// Whether audio is moving.
    pub playing: bool,
    /// How many lanes the wall falls back to before its first paint has
    /// measured a cross extent.
    pub fallback_lanes: usize,
}

impl WallLayout {
    /// How many tiles share a line at the current cross extent: enough that
    /// the configured edge covers it. The ceil keeps the actual edge at or
    /// under the configured one, so nothing upscales past the stored
    /// thumbnail.
    pub fn lanes(&self) -> usize {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return self.fallback_lanes;
        }
        let gap = self.gap;
        // The caption is below the tile, so while the wall scrolls
        // horizontally each tile's footprint along the height grows by it;
        // vertical packing is unchanged, the caption extends the line down
        // into the scroll instead of eating a lane.
        let footprint = self.tile + self.cross_label();
        (((cross + gap) / (footprint + gap)).ceil() as usize).max(1)
    }

    /// The caption's height when labels are on, else zero.
    pub fn label_height(&self) -> f32 {
        if self.labels {
            TILE_LABEL_H
        } else {
            0.
        }
    }

    /// The caption's share of the cross extent: a horizontal wall has to
    /// stack captions along the packing axis, a vertical wall sends them
    /// into the scroll.
    pub fn cross_label(&self) -> f32 {
        if self.vertical {
            0.
        } else {
            self.label_height()
        }
    }

    /// Which way the wall scrolls.
    pub fn axis(&self) -> Axis {
        if self.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The leading tile currently in view, for the saved layout: the list's
    /// first line spread back over the lanes. A restore still pending (the
    /// panel never painted) reports its own target, so an unshown panel
    /// round-trips its position instead of dropping to zero.
    ///
    /// `offset` is the scroll handle's raw offset, which runs negative as
    /// the list scrolls.
    pub fn first_cell(&self, restore: Option<usize>, offset: Point<Pixels>, cells: usize) -> usize {
        if let Some(cell) = restore {
            return cell;
        }
        let lanes = self.lanes();
        // The line pitch is the tile edge plus the gap, plus the caption on
        // a vertical wall where it trails each tile into the scroll. A
        // horizontal wall stacks the caption on the cross axis, so it stays
        // out of the scroll pitch. This has to match the item sizes the
        // virtual list lays out, or the restored cell drifts as you scroll.
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

    /// A tile's edge: the cross extent split evenly over the lanes with the
    /// gaps taken out, so the last lane ends at the panel edge instead of
    /// bleeding past it.
    pub fn tile_side(&self) -> Pixels {
        let cross = f32::from(self.cross);
        if cross <= 0. {
            return px(self.tile);
        }
        let lanes = self.lanes() as f32;
        px((((cross - self.gap * (lanes - 1.)) / lanes) - self.cross_label()).max(1.))
    }

    /// Whether tile `ix` is in the receded set: the tiles the focus
    /// effects push back. The hovered tile and the playing one are always
    /// exempt. Always mode pushes back every other tile; otherwise only the
    /// rest while audio moves.
    pub fn receded(&self, ix: usize) -> bool {
        if self.hovered == Some(ix) || self.playing_ix == Some(ix) {
            return false;
        }
        self.dim_always || self.playing
    }

    /// A tile's resting opacity under the dim mode: the configured floor for
    /// a receded tile, full otherwise.
    pub fn dim_target(&self, ix: usize) -> f32 {
        if self.dim_playing && self.receded(ix) {
            1.0 - self.dim / TILE_DIM_MAX
        } else {
            1.0
        }
    }

    /// Whether tile `ix` draws drained of color.
    pub fn desaturated(&self, ix: usize) -> bool {
        self.desaturate_playing && self.receded(ix)
    }

    /// How far page up and page down move the cursor, in tiles: a fixed
    /// number of lines, widened by however many lanes the wall is packing
    /// at its current size.
    pub fn page_step(&self) -> isize {
        (PAGE_LINES * self.lanes()) as isize
    }

    /// How far an arrow key moves the cursor, or `None` for a key that
    /// isn't a step on this wall. One tile along the line the arrow points
    /// down, a whole line across it, and which arrow is which flips with
    /// the wall's orientation: a vertical wall packs its lines left to
    /// right, a horizontal one top to bottom.
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

    /// A horizontal wall packs its lines top to bottom, so the pair swaps:
    /// up and down walk one tile, left and right jump a column.
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
        // The gaps come out of the cross extent, so the last lane ends at
        // the panel edge.
        assert!(layout.tile_side() <= px(160.));
        let side = f32::from(layout.tile_side());
        assert!((side * lanes as f32 + 8. * (lanes as f32 - 1.) - 600.).abs() < 0.01);
    }

    /// A caption only takes from the cross extent on a horizontal wall,
    /// where it stacks along the packing axis: the footprint grows, so
    /// fewer lanes fit. A vertical wall sends the caption into the scroll
    /// and packs exactly as it would bare.
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
        // Past the end it clamps onto the last cell.
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
