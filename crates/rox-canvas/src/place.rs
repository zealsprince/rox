//! An item's position and the limits on its size: the parts of a [`Place`]
//! that resolve against the parent's box.
//!
//! Each of x and y is units from the anchor's point, or a fraction of the
//! parent's box along that axis, which reads in JSON as a size does:
//! `{"stage": 0.25}`. A position in units saves as a bare number, as every
//! position did before fractions, so an old canvas reads and writes the
//! same bytes. z is units only: the parent's box has no depth to take a
//! fraction of.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::state::{Length, Place};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// x then y.
    pub xy: [Length; 2],
    /// Units toward the viewer.
    pub z: f32,
}

impl Default for Position {
    fn default() -> Self {
        Position::units([0.; 3])
    }
}

impl Position {
    pub const fn units([x, y, z]: [f32; 3]) -> Position {
        Position {
            xy: [Length::Units(x), Length::Units(y)],
            z,
        }
    }

    /// Units from the anchor's point against a parent box of `parent`.
    pub fn resolve(&self, parent: [f32; 2]) -> [f32; 3] {
        [
            self.xy[0].resolve(parent[0]),
            self.xy[1].resolve(parent[1]),
            self.z,
        ]
    }

    /// Moved by `by` units on x and y, each axis kept in its own unit.
    pub fn shifted(self, by: [f32; 2], parent: [f32; 2]) -> Position {
        Position {
            xy: [
                self.xy[0].shifted(by[0], parent[0]),
                self.xy[1].shifted(by[1], parent[1]),
            ],
            z: self.z,
        }
    }
}

impl Length {
    /// `units` long against `parent_axis`, in this length's own unit. A
    /// fraction of a parent with no length has nothing to divide by and stays
    /// as it was.
    pub fn with_units(self, units: f32, parent_axis: f32) -> Length {
        match self {
            Length::Units(_) => Length::Units(units),
            Length::Stage(_) if parent_axis > 0. => Length::Stage(units / parent_axis),
            Length::Stage(f) => Length::Stage(f),
        }
    }

    /// Longer by `by` units, in its own unit.
    pub fn shifted(self, by: f32, parent_axis: f32) -> Length {
        self.with_units(self.resolve(parent_axis) + by, parent_axis)
    }

    pub fn is_fraction(self) -> bool {
        matches!(self, Length::Stage(_))
    }
}

impl Place {
    /// The size against a parent box of `parent`, each side held to its
    /// limits. A minimum over its maximum wins, the way CSS settles it.
    pub fn size_in(&self, parent: [f32; 2]) -> [f32; 2] {
        [0, 1].map(|axis| self.limited(axis, self.size[axis].resolve(parent[axis])))
    }

    /// `units` held to side `axis`'s limits.
    pub fn limited(&self, axis: usize, units: f32) -> f32 {
        let mut units = units;
        if let Some(max) = self.max_size[axis] {
            units = units.min(max);
        }
        if let Some(min) = self.min_size[axis] {
            units = units.max(min);
        }
        units
    }

    /// Units from the anchor's point against a parent box of `parent`.
    pub fn offset_in(&self, parent: [f32; 2]) -> [f32; 3] {
        self.position.resolve(parent)
    }
}

/// No limit on either side, which is what a size saved before limits holds.
pub(crate) fn no_limits(limits: &[Option<f32>; 2]) -> bool {
    limits.iter().all(Option::is_none)
}

/// One of x and y as JSON: a bare number of units, or a tagged length.
#[derive(Deserialize)]
#[serde(untagged)]
enum Axis {
    Bare(f32),
    Tagged(Length),
}

impl From<Axis> for Length {
    fn from(axis: Axis) -> Length {
        match axis {
            Axis::Bare(n) => Length::Units(n),
            Axis::Tagged(length) => length,
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum AxisOut {
    Bare(f32),
    Tagged(Length),
}

impl Serialize for Position {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let axis = |length: Length| match length {
            Length::Units(n) => AxisOut::Bare(n),
            fraction => AxisOut::Tagged(fraction),
        };

        (axis(self.xy[0]), axis(self.xy[1]), self.z).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Position {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (x, y, z) = <(Axis, Axis, f32)>::deserialize(deserializer)?;

        Ok(Position {
            xy: [x.into(), y.into()],
            z,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::state::CanvasItem;

    #[test]
    fn a_position_in_units_saves_as_it_always_did() {
        let position = Position::units([10., -4.5, 30.]);
        let saved = serde_json::to_value(position).unwrap();

        assert_eq!(saved, json!([10.0, -4.5, 30.0]));
        assert_eq!(serde_json::from_value::<Position>(saved).unwrap(), position);
    }

    #[test]
    fn a_fraction_reads_and_writes_the_way_a_size_does() {
        let position = Position {
            xy: [Length::Stage(0.25), Length::Units(12.)],
            z: 0.,
        };
        let text = serde_json::to_string(&position).unwrap();
        assert_eq!(text, r#"[{"stage":0.25},12.0,0.0]"#);

        let back: Position = serde_json::from_str(r#"[{"stage":0.25},{"units":12},0]"#).unwrap();
        assert_eq!(back, position);
    }

    #[test]
    fn a_fractional_depth_doesnt_parse() {
        assert!(serde_json::from_value::<Position>(json!([0, 0, { "stage": 0.5 }])).is_err());
        assert!(serde_json::from_value::<Position>(json!([0, 0])).is_err());
    }

    #[test]
    fn a_fraction_resolves_against_its_own_axis() {
        let position = Position {
            xy: [Length::Stage(0.5), Length::Stage(0.1)],
            z: 7.,
        };

        assert_eq!(position.resolve([800., 600.]), [400., 60., 7.]);
        assert_eq!(position.resolve([1600., 900.]), [800., 90., 7.]);
    }

    #[test]
    fn a_shift_keeps_each_axis_in_its_unit() {
        let position = Position {
            xy: [Length::Stage(0.5), Length::Units(10.)],
            z: 3.,
        };
        let moved = position.shifted([80., 5.], [800., 600.]);

        assert_eq!(moved.xy, [Length::Stage(0.6), Length::Units(15.)]);
        assert_eq!(moved.z, 3.);

        // No box to measure a fraction against leaves it where it was.
        assert_eq!(
            Length::Stage(0.3).shifted(40., 0.),
            Length::Stage(0.3),
            "a parent with no width"
        );
    }

    #[test]
    fn limits_hold_a_fractional_size_and_a_minimum_wins_a_tie() {
        let mut place = Place {
            size: [Length::Stage(0.5), Length::Stage(0.25)],
            min_size: [Some(300.), None],
            max_size: [Some(600.), Some(100.)],
            ..Place::default()
        };

        assert_eq!(place.size_in([400., 800.]), [300., 100.]);
        assert_eq!(place.size_in([1000., 200.]), [500., 50.]);
        assert_eq!(place.size_in([2000., 800.]), [600., 100.]);

        place.min_size[0] = Some(700.);
        assert_eq!(place.size_in([2000., 800.])[0], 700.);
    }

    #[test]
    fn a_place_without_limits_saves_without_them() {
        let saved = serde_json::to_value(Place::default()).unwrap();
        assert!(saved.get("min_size").is_none(), "{saved}");
        assert!(saved.get("max_size").is_none(), "{saved}");

        let limited = Place {
            min_size: [Some(120.), None],
            ..Place::default()
        };
        let saved = serde_json::to_value(limited).unwrap();
        assert_eq!(saved["min_size"], json!([120.0, null]));
        assert!(saved.get("max_size").is_none());
        assert_eq!(serde_json::from_value::<Place>(saved).unwrap(), limited);
    }

    #[test]
    fn an_item_saved_before_fractions_reads_and_saves_byte_for_byte() {
        let saved = r#"{"id":"a","name":"","place":{"anchor":"top_left","position":[12.5,-3.0,40.0],"size":[{"stage":0.5},{"units":200.0}],"rotation":[0.0,10.0,0.0],"scale":1.0},"visible":true,"locked":false,"opacity":1.0,"content":{"kind":"image","asset":"x","fit":"stretch"}}"#;
        let item: CanvasItem = serde_json::from_str(saved).unwrap();

        assert_eq!(item.place.position, Position::units([12.5, -3., 40.]));
        assert_eq!(serde_json::to_string(&item).unwrap(), saved);
    }
}
