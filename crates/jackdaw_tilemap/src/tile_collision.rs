//! Tile collision: shapes on numbered physics layers, each solid or a
//! trigger, drawn from a preset or traced from the tile's art.

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
}

impl ShapePreset {
    pub const ALL: [ShapePreset; 6] = [
        ShapePreset::Solid,
        ShapePreset::TopHalf,
        ShapePreset::BottomHalf,
        ShapePreset::SlopeUp,
        ShapePreset::SlopeDown,
        ShapePreset::Traced,
    ];

    /// The preset's polygons, counter-clockwise in tile fractions, y up,
    /// origin at the tile's bottom-left; `None` for a traced shape, which
    /// depends on the art.
    pub fn polygons(self) -> Option<Vec<Vec<Vec2>>> {
        let ring = |points: &[(f32, f32)]| points.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
        let shape = match self {
            ShapePreset::Solid => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
            ShapePreset::TopHalf => ring(&[(0.0, 0.5), (1.0, 0.5), (1.0, 1.0), (0.0, 1.0)]),
            ShapePreset::BottomHalf => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5)]),
            ShapePreset::SlopeUp => ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]),
            ShapePreset::SlopeDown => ring(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]),
            ShapePreset::Traced => return None,
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
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// A tile's collision shape on one physics layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileCollision {
    pub layer: u32,
    /// Overlaps without blocking.
    pub trigger: bool,
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
            preset: ShapePreset::Traced,
            polygons: shape.polygons,
        }
    }

    /// Preset `preset` on `layer`; `None` for a traced shape.
    pub fn preset(preset: ShapePreset, layer: u32, trigger: bool) -> Option<Self> {
        Some(Self {
            layer,
            trigger,
            preset,
            polygons: preset.polygons()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(ring: &[Vec2]) -> f32 {
        let next = ring.iter().cycle().skip(1);
        ring.iter()
            .zip(next)
            .map(|(a, b)| a.perp_dot(*b))
            .sum::<f32>()
            / 2.0
    }

    #[test]
    fn every_preset_but_traced_is_counter_clockwise_inside_the_tile() {
        for preset in ShapePreset::ALL {
            let Some(polygons) = preset.polygons() else {
                assert_eq!(preset, ShapePreset::Traced);
                continue;
            };
            for ring in &polygons {
                assert!(area(ring) > 0.0, "{preset:?}");
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
        assert_eq!(ShapePreset::from_name("custom"), None);
    }
}
