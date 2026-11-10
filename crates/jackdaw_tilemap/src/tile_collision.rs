//! Tile collision: shapes on numbered physics layers, each solid or a
//! trigger, made from a preset, traced from the tile's art or drawn point
//! by point.

use glam::Vec2;

use crate::TileShape;

/// What a collision shape was made from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShapePreset {
    /// The whole tile.
    #[default]
    Solid,
    TopHalf,
    BottomHalf,
    /// A slope rising to the right.
    SlopeUp,
    /// A slope falling to the right.
    SlopeDown,
    /// The tile's opaque pixels.
    Traced,
    /// Polygons drawn point by point.
    Custom,
}

impl ShapePreset {
    pub const ALL: [ShapePreset; 7] = [
        ShapePreset::Solid,
        ShapePreset::TopHalf,
        ShapePreset::BottomHalf,
        ShapePreset::SlopeUp,
        ShapePreset::SlopeDown,
        ShapePreset::Traced,
        ShapePreset::Custom,
    ];

    /// The preset's polygons, counter-clockwise in tile fractions, y up,
    /// origin at the tile's bottom-left; `None` for a traced or custom
    /// shape, which depends on the art or the hand.
    pub fn polygons(self) -> Option<Vec<Vec<Vec2>>> {
        let ring = |points: &[(f32, f32)]| points.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
        let shape = match self {
            ShapePreset::Solid => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
            ShapePreset::TopHalf => ring(&[(0.0, 0.5), (1.0, 0.5), (1.0, 1.0), (0.0, 1.0)]),
            ShapePreset::BottomHalf => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5)]),
            ShapePreset::SlopeUp => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]),
            ShapePreset::SlopeDown => ring(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]),
            ShapePreset::Traced | ShapePreset::Custom => return None,
        };
        Some(vec![shape])
    }

    /// The name operators and text take.
    pub fn name(self) -> &'static str {
        match self {
            ShapePreset::Solid => "solid",
            ShapePreset::TopHalf => "top_half",
            ShapePreset::BottomHalf => "bottom_half",
            ShapePreset::SlopeUp => "slope_up",
            ShapePreset::SlopeDown => "slope_down",
            ShapePreset::Traced => "traced",
            ShapePreset::Custom => "custom",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The preset that draws `polygons`, whatever point each ring
    /// starts at; custom for any other shape.
    pub fn of_polygons(polygons: &[Vec<Vec2>]) -> Self {
        let same_ring = |a: &[Vec2], b: &[Vec2]| {
            a.len() == b.len()
                && (0..a.len()).any(|shift| {
                    (a.iter().zip(b.iter().cycle().skip(shift)))
                        .all(|(p, q)| p.abs_diff_eq(*q, 1e-5))
                })
        };
        let matches = |preset: &ShapePreset| {
            preset.polygons().is_some_and(|rings| {
                rings.len() == polygons.len()
                    && (rings.iter().zip(polygons)).all(|(a, b)| same_ring(a, b))
            })
        };
        (Self::ALL.into_iter())
            .find(matches)
            .unwrap_or(ShapePreset::Custom)
    }
}

/// A turn of a whole collision shape inside its tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeTurn {
    FlipX,
    FlipY,
    /// A quarter turn clockwise.
    Rotate,
}

impl ShapeTurn {
    /// `polygons`, in tile fractions y up, turned about the tile's center;
    /// each ring stays counter-clockwise.
    pub fn apply(self, polygons: &[Vec<Vec2>]) -> Vec<Vec<Vec2>> {
        let at = |p: Vec2| match self {
            ShapeTurn::FlipX => Vec2::new(1.0 - p.x, p.y),
            ShapeTurn::FlipY => Vec2::new(p.x, 1.0 - p.y),
            ShapeTurn::Rotate => Vec2::new(p.y, 1.0 - p.x),
        };
        (polygons.iter())
            .map(|ring| {
                let mut turned: Vec<Vec2> = ring.iter().map(|p| at(*p)).collect();
                if self != ShapeTurn::Rotate {
                    turned.reverse();
                }
                turned
            })
            .collect()
    }
}

/// Twice the signed area of `ring`, positive when counter-clockwise with y
/// up.
pub fn ring_area(ring: &[Vec2]) -> f32 {
    let next = ring.iter().cycle().skip(1);
    ring.iter().zip(next).map(|(a, b)| a.perp_dot(*b)).sum()
}

/// A tile's collision shape on one physics layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileCollision {
    pub layer: u32,
    /// Overlaps without blocking.
    pub trigger: bool,
    /// Blocks only what comes down onto it from above.
    pub one_way: bool,
    pub preset: ShapePreset,
    /// Counter-clockwise outer rings in tile fractions, y up, origin at the
    /// tile's bottom-left.
    pub polygons: Vec<Vec<Vec2>>,
}

impl TileCollision {
    /// A traced shape as solid collision on layer 0.
    pub fn traced(shape: TileShape) -> Self {
        Self {
            layer: 0,
            trigger: false,
            one_way: false,
            preset: ShapePreset::Traced,
            polygons: shape.polygons,
        }
    }

    /// Preset `preset` on `layer`; `None` for a traced or custom shape.
    pub fn preset(preset: ShapePreset, layer: u32, trigger: bool) -> Option<Self> {
        Some(Self {
            layer,
            trigger,
            one_way: false,
            preset,
            polygons: preset.polygons()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_but_traced_and_custom_is_counter_clockwise_inside_the_tile() {
        for preset in ShapePreset::ALL {
            let Some(polygons) = preset.polygons() else {
                assert!(matches!(preset, ShapePreset::Traced | ShapePreset::Custom));
                continue;
            };
            for ring in &polygons {
                assert!(ring_area(ring) > 0.0, "{preset:?}");
                let inside = |v: &Vec2| v.cmpge(Vec2::ZERO).all() && v.cmple(Vec2::ONE).all();
                assert!(ring.iter().all(inside), "{preset:?}");
            }
        }
    }

    #[test]
    fn a_preset_name_reads_back() {
        for preset in ShapePreset::ALL {
            assert_eq!(ShapePreset::from_name(preset.name()), Some(preset));
        }
        assert_eq!(ShapePreset::from_name("round"), None);
    }

    #[test]
    fn a_shape_reads_as_the_preset_it_draws_from_any_starting_point() {
        let mut ring = ShapePreset::TopHalf.polygons().expect("a ring").remove(0);
        ring.rotate_left(2);
        assert_eq!(ShapePreset::of_polygons(&[ring]), ShapePreset::TopHalf);
        let notch = vec![
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.0, 1.0),
        ];
        assert_eq!(ShapePreset::of_polygons(&[notch]), ShapePreset::Custom);
        assert_eq!(ShapePreset::of_polygons(&[]), ShapePreset::Custom);
    }

    #[test]
    fn turning_a_slope_gives_the_other_slope_still_counter_clockwise() {
        let up = ShapePreset::SlopeUp.polygons().expect("a ring");
        for turn in [ShapeTurn::FlipX, ShapeTurn::Rotate] {
            let turned = turn.apply(&up);
            assert!(ring_area(&turned[0]) > 0.0, "{turn:?}");
            assert_eq!(ShapePreset::of_polygons(&turned), ShapePreset::SlopeDown);
        }
        let flipped = ShapeTurn::FlipY.apply(&up);
        assert!(ring_area(&flipped[0]) > 0.0);
        assert_eq!(ShapePreset::of_polygons(&flipped), ShapePreset::Custom);
        let solid = ShapePreset::Solid.polygons().expect("a ring");
        let once = ShapeTurn::Rotate.apply(&solid);
        assert_eq!(ShapePreset::of_polygons(&once), ShapePreset::Solid);
    }
}
