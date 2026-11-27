//! An item's position and the limits on its size: the parts of a [`Place`]
//! that resolve against the parent's box.
//!
//! Each of x and y is units from the anchor's point, or a fraction of the
//! parent's box along that axis, which reads in JSON as a size does:
//! `{"stage": 0.25}`, or both at once: `{"stage": -0.44, "units": -17}`. A
//! position in units saves as a bare number, as every position did before
//! fractions, so an old canvas reads and writes the same bytes. z is units
//! only: the parent's box has no depth to take a fraction of.
//!
//! A mixed length is its own form, kept even while its units are zero, so a
//! drag through zero never turns it into a bare fraction.

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

/// Which of the three forms a [`Length`] takes, for the inspector's choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthKind {
    Units,
    Stage,
    Mixed,
}

impl Length {
    /// `units` long against `parent_axis`, in this length's own unit. A
    /// fraction of a parent with no length has nothing to divide by and stays
    /// as it was. A mixed length keeps its fraction and its units take the
    /// difference, so it goes on following the parent the same way.
    pub fn with_units(self, units: f32, parent_axis: f32) -> Length {
        match self {
            Length::Units(_) => Length::Units(units),
            Length::Stage(_) if parent_axis > 0. => Length::Stage(units / parent_axis),
            Length::Stage(f) => Length::Stage(f),
            Length::Mixed { stage, .. } => Length::Mixed {
                stage,
                units: units - stage * parent_axis,
            },
        }
    }

    /// Longer by `by` units, in its own unit.
    pub fn shifted(self, by: f32, parent_axis: f32) -> Length {
        self.with_units(self.resolve(parent_axis) + by, parent_axis)
    }

    /// Zero in its own form: units or a fraction of none.
    pub fn zeroed(self) -> Length {
        match self {
            Length::Units(_) => Length::Units(0.),
            Length::Stage(_) => Length::Stage(0.),
            Length::Mixed { .. } => Length::Mixed {
                stage: 0.,
                units: 0.,
            },
        }
    }

    /// Whether it grows with the parent: a fraction, alone or with units.
    pub fn is_fraction(self) -> bool {
        matches!(self, Length::Stage(_) | Length::Mixed { .. })
    }

    pub fn kind(self) -> LengthKind {
        match self {
            Length::Units(_) => LengthKind::Units,
            Length::Stage(_) => LengthKind::Stage,
            Length::Mixed { .. } => LengthKind::Mixed,
        }
    }

    /// The same length against `parent_axis` in another form. A fraction or
    /// units gaining the other term starts it at zero, so nothing moves and
    /// what it had stays as typed. A parent with no length has no fraction
    /// to give, so a switch to a bare fraction there leaves it as it was.
    pub fn with_kind(self, kind: LengthKind, parent_axis: f32) -> Length {
        let resolved = self.resolve(parent_axis);

        match (kind, self) {
            (LengthKind::Units, _) => Length::Units(resolved),

            (LengthKind::Stage, Length::Stage(_)) => self,
            (LengthKind::Stage, _) if parent_axis > 0. => Length::Stage(resolved / parent_axis),
            (LengthKind::Stage, _) => self,

            (LengthKind::Mixed, Length::Units(units)) => Length::Mixed { stage: 0., units },
            (LengthKind::Mixed, Length::Stage(stage)) => Length::Mixed { stage, units: 0. },
            (LengthKind::Mixed, Length::Mixed { .. }) => self,
        }
    }
}

impl Serialize for Length {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let (stage, units) = match *self {
            Length::Units(n) => (None, Some(n)),
            Length::Stage(f) => (Some(f), None),
            Length::Mixed { stage, units } => (Some(stage), Some(units)),
        };

        let mut map = serializer.serialize_map(None)?;
        if let Some(stage) = stage {
            map.serialize_entry("stage", &stage)?;
        }
        if let Some(units) = units {
            map.serialize_entry("units", &units)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(LengthVisitor)
    }
}

struct LengthVisitor;

impl<'de> serde::de::Visitor<'de> for LengthVisitor {
    type Value = Length;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(r#"a length: {"units": n}, {"stage": f}, or both"#)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Length, A::Error> {
        use serde::de::Error;

        let (mut stage, mut units) = (None, None);
        while let Some(key) = map.next_key::<String>()? {
            let slot = match key.as_str() {
                "stage" => &mut stage,
                "units" => &mut units,
                _ => return Err(A::Error::unknown_field(&key, &["stage", "units"])),
            };
            if slot.is_some() {
                return Err(A::Error::custom(format!("`{key}` twice in one length")));
            }
            *slot = Some(map.next_value::<f32>()?);
        }

        match (stage, units) {
            (Some(stage), Some(units)) => Ok(Length::Mixed { stage, units }),
            (Some(stage), None) => Ok(Length::Stage(stage)),
            (None, Some(units)) => Ok(Length::Units(units)),
            (None, None) => Err(A::Error::custom(
                "a length with neither `stage` nor `units`",
            )),
        }
    }
}

impl Place {
    /// The size against a parent box of `parent`, each side held to its
    /// limits. A minimum over its maximum wins, the way CSS settles it. With
    /// an aspect the box is then the largest of that shape inside it.
    pub fn size_in(&self, parent: [f32; 2]) -> [f32; 2] {
        let space = self.space_in(parent);
        match self.aspect() {
            Some(aspect) => fitted(space, aspect),
            None => space,
        }
    }

    /// The box the sizes and limits give, before any aspect: the space a
    /// locked shape fits inside.
    pub fn space_in(&self, parent: [f32; 2]) -> [f32; 2] {
        [0, 1].map(|axis| self.limited(axis, self.size[axis].resolve(parent[axis])))
    }

    /// The locked width over height, if there's one that can be drawn. A
    /// hand-edited zero, a negative or a NaN reads as no lock.
    pub fn aspect(&self) -> Option<f32> {
        self.aspect.filter(|a| a.is_finite() && *a > 0.)
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

/// The largest box `aspect` wide over high that fits inside `space`. Since
/// the anchor places a box by its own matching point, the fitted box sits at
/// the anchor's spot inside the space: a corner anchor keeps that corner, a
/// centred one centres it. A space with no area stays as it is.
pub fn fitted(space: [f32; 2], aspect: f32) -> [f32; 2] {
    let [width, height] = space;
    if !(width > 0. && height > 0.) {
        return space;
    }

    if width / height > aspect {
        [height * aspect, height]
    } else {
        [width, width / aspect]
    }
}

/// No limit on either side, which is what a size saved before limits holds.
pub(crate) fn no_limits(limits: &[Option<f32>; 2]) -> bool {
    limits.iter().all(Option::is_none)
}

/// No growth, which is what a place saved before stacks could grow holds.
pub(crate) fn no_growth(grow: &f32) -> bool {
    *grow == 0.
}

pub(crate) fn no_margin(margin: &[f32; 2]) -> bool {
    *margin == [0.; 2]
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
    fn a_locked_box_is_the_largest_of_its_shape_inside_the_sizes() {
        let place = Place {
            size: [Length::Stage(0.5), Length::Stage(0.5)],
            aspect: Some(1.),
            ..Place::default()
        };

        // Wide, square and tall windows: always square, as big as fits.
        assert_eq!(place.size_in([1600., 900.]), [450., 450.]);
        assert_eq!(place.size_in([1000., 1000.]), [500., 500.]);
        assert_eq!(place.size_in([600., 1200.]), [300., 300.]);
        assert_eq!(place.space_in([600., 1200.]), [300., 600.]);

        // Wider than high.
        assert_eq!(fitted([400., 400.], 2.), [400., 200.]);
        assert_eq!(fitted([1000., 100.], 2.), [200., 100.]);

        // No area to fit inside stays as it is.
        assert_eq!(fitted([0., 300.], 1.), [0., 300.]);
    }

    #[test]
    fn the_limits_bound_the_space_and_the_shape_fits_inside_them() {
        let place = Place {
            size: [Length::Stage(0.5), Length::Stage(0.9)],
            max_size: [None, Some(460.)],
            min_size: [Some(300.), None],
            aspect: Some(1.),
            ..Place::default()
        };

        // The height's maximum holds the space to 460 high.
        assert_eq!(place.size_in([2000., 1000.]), [460., 460.]);

        // A minimum widens the space, and the square fits the narrower side,
        // so the minimum bounds the space, not the drawn side.
        assert_eq!(place.space_in([400., 200.]), [300., 180.]);
        assert_eq!(place.size_in([400., 200.]), [180., 180.]);
    }

    #[test]
    fn an_aspect_that_cant_be_drawn_reads_as_no_lock() {
        for junk in [0., -1., f32::NAN, f32::INFINITY] {
            let place = Place {
                aspect: Some(junk),
                ..Place::default()
            };
            assert_eq!(place.aspect(), None);
            assert_eq!(place.size_in([800., 600.]), [480., 320.]);
        }
    }

    #[test]
    fn a_locked_place_saves_its_aspect_and_an_unlocked_one_doesnt() {
        let saved = serde_json::to_value(Place::default()).unwrap();
        assert!(saved.get("aspect").is_none(), "{saved}");

        let locked = Place {
            aspect: Some(1.5),
            ..Place::default()
        };
        let saved = serde_json::to_value(locked).unwrap();
        assert_eq!(saved["aspect"], json!(1.5));
        assert_eq!(serde_json::from_value::<Place>(saved).unwrap(), locked);
    }

    #[test]
    fn a_mixed_length_reads_and_writes_both_terms() {
        let saved = r#"{"id":"frame","name":"","place":{"anchor":"center","position":[{"stage":-0.44,"units":-17.0},90.0,-60.0],"size":[{"stage":1.0,"units":48.0},{"stage":1.0,"units":-214.0}],"rotation":[0.0,0.0,0.0],"scale":1.0},"visible":true,"locked":false,"opacity":1.0,"content":{"kind":"image","asset":"x","fit":"stretch"}}"#;
        let item: CanvasItem = serde_json::from_str(saved).unwrap();

        assert_eq!(
            item.place.position.xy[0],
            Length::Mixed {
                stage: -0.44,
                units: -17.
            }
        );
        assert_eq!(
            item.place.size,
            [
                Length::Mixed {
                    stage: 1.,
                    units: 48.
                },
                Length::Mixed {
                    stage: 1.,
                    units: -214.
                }
            ]
        );
        assert_eq!(serde_json::to_string(&item).unwrap(), saved);

        // Units first or bare integers read the same, and zero units stay.
        let back: Length = serde_json::from_str(r#"{"units":0,"stage":1}"#).unwrap();
        assert_eq!(
            back,
            Length::Mixed {
                stage: 1.,
                units: 0.
            }
        );
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            r#"{"stage":1.0,"units":0.0}"#
        );
    }

    #[test]
    fn a_length_with_a_key_it_doesnt_know_or_none_doesnt_parse() {
        for junk in [
            json!({}),
            json!({ "stage": 0.5, "pixels": 3 }),
            json!({ "percent": 50 }),
            json!({ "units": "wide" }),
            json!(12),
        ] {
            assert!(
                serde_json::from_value::<Length>(junk.clone()).is_err(),
                "{junk}"
            );
        }
    }

    #[test]
    fn a_mixed_length_follows_the_parent_by_its_fraction_and_keeps_its_units() {
        let frame = Length::Mixed {
            stage: 1.,
            units: 48.,
        };

        // The margin past the parent holds at any size.
        for parent in [1100., 1920., 900.] {
            assert_eq!(frame.resolve(parent) - parent, 48.);
        }

        // A move lands on the units, so it goes on following the same way.
        let moved = frame.shifted(-8., 1000.);
        assert_eq!(
            moved,
            Length::Mixed {
                stage: 1.,
                units: 40.
            }
        );
        assert_eq!(
            frame.with_units(1000., 1000.),
            Length::Mixed {
                stage: 1.,
                units: 0.
            },
            "zero units keep the form"
        );

        // A parent with no length: only the units can move.
        assert_eq!(
            frame.with_units(30., 0.),
            Length::Mixed {
                stage: 1.,
                units: 30.
            }
        );
        assert!(frame.is_fraction());
    }

    #[test]
    fn switching_form_keeps_the_length() {
        let parent = 800.;
        let forms = [
            Length::Units(200.),
            Length::Stage(0.25),
            Length::Mixed {
                stage: 0.2,
                units: 40.,
            },
        ];

        for length in forms {
            for kind in [LengthKind::Units, LengthKind::Stage, LengthKind::Mixed] {
                let switched = length.with_kind(kind, parent);
                assert_eq!(switched.kind(), kind, "{length:?} to {kind:?}");
                assert!(
                    (switched.resolve(parent) - 200.).abs() < 1e-3,
                    "{length:?} to {kind:?}: {switched:?}"
                );
            }
        }

        // Gaining a term starts it at zero and keeps what was typed.
        assert_eq!(
            Length::Units(48.).with_kind(LengthKind::Mixed, parent),
            Length::Mixed {
                stage: 0.,
                units: 48.
            }
        );
        assert_eq!(
            Length::Stage(1.).with_kind(LengthKind::Mixed, parent),
            Length::Mixed {
                stage: 1.,
                units: 0.
            }
        );

        // No length to divide by: a bare fraction can't be made.
        assert_eq!(
            Length::Units(5.).with_kind(LengthKind::Stage, 0.),
            Length::Units(5.)
        );
    }

    #[test]
    fn limits_and_aspect_hold_a_mixed_size() {
        let place = Place {
            size: [
                Length::Mixed {
                    stage: 0.5,
                    units: -20.,
                },
                Length::Mixed {
                    stage: 1.,
                    units: -100.,
                },
            ],
            max_size: [Some(500.), None],
            aspect: Some(1.),
            ..Place::default()
        };

        assert_eq!(place.space_in([800., 600.]), [380., 500.]);
        assert_eq!(place.size_in([800., 600.]), [380., 380.]);
        assert_eq!(place.space_in([2000., 600.]), [500., 500.]);
        assert_eq!(place.size_in([2000., 400.]), [300., 300.]);
    }

    #[test]
    fn an_item_saved_before_fractions_reads_and_saves_byte_for_byte() {
        let saved = r#"{"id":"a","name":"","place":{"anchor":"top_left","position":[12.5,-3.0,40.0],"size":[{"stage":0.5},{"units":200.0}],"rotation":[0.0,10.0,0.0],"scale":1.0},"visible":true,"locked":false,"opacity":1.0,"content":{"kind":"image","asset":"x","fit":"stretch"}}"#;
        let item: CanvasItem = serde_json::from_str(saved).unwrap();

        assert_eq!(item.place.position, Position::units([12.5, -3., 40.]));
        assert_eq!(serde_json::to_string(&item).unwrap(), saved);
    }
}
