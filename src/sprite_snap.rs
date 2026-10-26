//! Where a sprite drag on a 2D world stage comes to rest. Plain arithmetic
//! over world units (y up): the gizmo gathers the other sprites' bounds and
//! the canvas settings, and [`jackdaw_snap::land_2d`] decides the landing,
//! the same pipeline the UI canvas runs.
//!
//! Each axis is decided on its own, first match wins: another sprite's edge,
//! centre or a guide within the radius, then the grid, then the pixel. Grid and pixel
//! both put the moving bounds' lower corner on their lattice, so a pixel snap
//! keeps a sprite's texel edges on whole pixels whatever its anchor. With the
//! magnet off nothing snaps.

use bevy::math::{BVec2, Rect, Vec2};
use jackdaw_snap::{SnapRect, land_2d};

use crate::canvas_snap::CanvasSnap;

/// World units per sprite pixel. No project setting names another density
/// yet, so a pixel is one world unit.
pub const WORLD_UNITS_PER_PIXEL: f32 = 1.0;

/// How far, in world units, a scaled edge may stop short of the line it
/// landed on and still count as on it.
const REACH_TOLERANCE: f32 = 1e-3;

/// The lines a drag can land on, per axis, in world units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpriteSnapTargets {
    /// The edges and centres of the sprites that are not moving.
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    /// The scene's guides: the vertical ones' x and the horizontal ones' y.
    pub guide_x: Vec<f32>,
    pub guide_y: Vec<f32>,
}

impl SpriteSnapTargets {
    /// The edges and centres of `rects`.
    pub fn from_rects(rects: impl IntoIterator<Item = Rect>) -> Self {
        let mut targets = Self::default();
        for rect in rects {
            let centre = rect.center();
            targets.x.extend([rect.min.x, centre.x, rect.max.x]);
            targets.y.extend([rect.min.y, centre.y, rect.max.y]);
        }
        targets
    }
}

/// What one drag event may snap to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteSnapRules {
    /// Whether anything snaps at all.
    pub magnet: bool,
    /// The lattice in world units, when grid snapping is on.
    pub grid: Option<f32>,
    /// The pixel in world units, when pixel snapping is on.
    pub pixel: Option<f32>,
    /// Whether the other sprites' edges and centres are offered.
    pub edges: bool,
    /// Whether the scene's guides are offered.
    pub guides: bool,
    /// How near, in world units, a line has to be to land on it.
    pub radius: f32,
}

impl SpriteSnapRules {
    /// The rules `kinds` set for a drag with the magnet `magnet`, on a panel
    /// whose grid is `grid` world units.
    pub fn new(kinds: &CanvasSnap, magnet: bool, grid: f32, radius: f32) -> Self {
        Self {
            magnet,
            grid: (kinds.grid && grid > 0.0 && grid.is_finite()).then_some(grid),
            pixel: kinds.pixel.then_some(WORLD_UNITS_PER_PIXEL),
            edges: kinds.sprite_edges,
            guides: kinds.snaps_to_guides(),
            radius,
        }
    }

    /// These rules less the sprite edges, for a drag whose size is unknown.
    /// The guides stay: they are lines in the world, not a sprite's size.
    pub fn without_edges(self) -> Self {
        Self {
            edges: false,
            ..self
        }
    }
}

/// Where one drag event landed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpriteLanding {
    /// How far the drag moves on top of the pointer's own delta.
    pub nudge: Vec2,
    /// The x of the line the x axis landed on, if any.
    pub x: Option<f32>,
    /// The y of the line the y axis landed on, if any.
    pub y: Option<f32>,
}

impl SpriteLanding {
    /// This landing less any line the dragged edge, having come to rest at
    /// `reached`, does not actually sit on: that axis neither lands nor
    /// moves for it.
    pub fn keep_reached(self, reached: Vec2) -> Self {
        let missed = |line: Option<f32>, at: f32| {
            line.is_some_and(|line| (line - at).abs() > REACH_TOLERANCE)
        };
        let mut kept = self;
        if missed(self.x, reached.x) {
            kept.x = None;
            kept.nudge.x = 0.0;
        }
        if missed(self.y, reached.y) {
            kept.y = None;
            kept.nudge.y = 0.0;
        }
        kept
    }
}

/// The landing of `moving`, the dragged bounds (or a point, for a scale
/// handle or a drag of unknown size), on the axes `axes` names. An axis left
/// out neither moves nor lands.
pub fn land(
    moving: Rect,
    axes: BVec2,
    targets: &SpriteSnapTargets,
    rules: &SpriteSnapRules,
) -> SpriteLanding {
    if !rules.magnet {
        return SpriteLanding::default();
    }
    let offered = |edges: &[f32], guides: &[f32]| -> Vec<f32> {
        let edges = edges.iter().filter(|_| rules.edges);
        let guides = guides.iter().filter(|_| rules.guides);
        edges.chain(guides).copied().collect()
    };
    let x = offered(&targets.x, &targets.guide_x);
    let y = offered(&targets.y, &targets.guide_y);
    let landing = land_2d(
        SnapRect {
            min: moving.min,
            max: moving.max,
        },
        axes,
        &x,
        &y,
        rules.radius,
        rules.grid.or(rules.pixel).unwrap_or(0.0),
    );
    SpriteLanding {
        nudge: landing.nudge,
        x: landing.x.map(|won| x[won.candidate]),
        y: landing.y.map(|won| y[won.candidate]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(grid: Option<f32>, pixel: bool) -> SpriteSnapRules {
        SpriteSnapRules {
            magnet: true,
            grid,
            pixel: pixel.then_some(1.0),
            edges: true,
            guides: true,
            radius: 3.0,
        }
    }

    fn square(min: Vec2) -> Rect {
        Rect::from_corners(min, min + Vec2::splat(10.0))
    }

    #[test]
    fn pixel_lands_the_lower_corner_on_a_whole_unit() {
        let moving = Rect::from_corners(Vec2::new(0.3, 0.6), Vec2::new(15.3, 15.6));
        let landing = land(
            moving,
            BVec2::TRUE,
            &SpriteSnapTargets::default(),
            &rules(None, true),
        );
        assert!((landing.nudge - Vec2::new(-0.3, 0.4)).length() < 1e-4);
        assert_eq!((landing.x, landing.y), (None, None));
    }

    #[test]
    fn grid_lands_the_lower_corner_and_outranks_the_pixel() {
        let landing = land(
            square(Vec2::new(13.0, -5.0)),
            BVec2::TRUE,
            &SpriteSnapTargets::default(),
            &rules(Some(8.0), true),
        );
        assert_eq!(landing.nudge, Vec2::new(3.0, -3.0));
    }

    #[test]
    fn an_odd_sprite_on_the_grid_keeps_whole_edges() {
        let moving = Rect::from_corners(Vec2::new(5.5, -9.5), Vec2::new(20.5, 5.5));
        let landing = land(
            moving,
            BVec2::TRUE,
            &SpriteSnapTargets::default(),
            &rules(Some(8.0), true),
        );
        let min = moving.min + landing.nudge;
        let max = moving.max + landing.nudge;
        assert_eq!(min, Vec2::new(8.0, -8.0));
        assert_eq!(max, Vec2::new(23.0, 7.0));
    }

    #[test]
    fn a_landing_the_edge_never_reached_is_dropped() {
        let landing = SpriteLanding {
            nudge: Vec2::new(1.0, 1.0),
            x: Some(10.0),
            y: Some(20.0),
        };
        let kept = landing.keep_reached(Vec2::new(10.0, 19.8));
        assert_eq!(kept.x, Some(10.0));
        assert_eq!(kept.y, None);
        assert_eq!(kept.nudge, Vec2::new(1.0, 0.0));
    }

    #[test]
    fn an_edge_in_reach_outranks_the_grid_and_names_its_line() {
        let targets = SpriteSnapTargets::from_rects([square(Vec2::new(22.0, 40.0))]);
        let landing = land(
            square(Vec2::new(10.5, 1.0)),
            BVec2::TRUE,
            &targets,
            &rules(Some(8.0), false),
        );
        assert_eq!(
            landing.nudge.x, 1.5,
            "the right edge lands on the left edge"
        );
        assert_eq!(landing.x, Some(22.0));
        assert_eq!(
            landing.nudge.y, -1.0,
            "y is out of reach, so the grid has it"
        );
        assert_eq!(landing.y, None);
    }

    #[test]
    fn edges_and_centres_both_offer_a_line() {
        let targets = SpriteSnapTargets::from_rects([square(Vec2::new(100.0, 100.0))]);
        let landing = land(
            square(Vec2::new(-20.0, 99.0)),
            BVec2::TRUE,
            &targets,
            &rules(None, false),
        );
        assert_eq!(landing.y, Some(100.0), "the bottom edge is nearest");
        assert_eq!(landing.nudge, Vec2::new(0.0, 1.0));
        let moved = land(
            square(Vec2::new(-20.0, 104.0)),
            BVec2::TRUE,
            &targets,
            &rules(None, false),
        );
        assert_eq!(moved.y, Some(105.0));
    }

    #[test]
    fn an_axis_left_out_neither_moves_nor_lands() {
        let targets = SpriteSnapTargets::from_rects([square(Vec2::new(11.0, 11.0))]);
        let landing = land(
            square(Vec2::new(0.4, 0.4)),
            BVec2::new(true, false),
            &targets,
            &rules(None, true),
        );
        assert_eq!(landing.x, Some(11.0));
        assert_eq!(landing.y, None);
        assert_eq!(landing.nudge.y, 0.0);
    }

    #[test]
    fn edges_and_guides_are_offered_on_their_own() {
        let targets = SpriteSnapTargets {
            guide_x: vec![30.0],
            ..SpriteSnapTargets::from_rects([square(Vec2::new(11.0, 50.0))])
        };
        let point = Rect::from_corners(Vec2::new(10.5, 0.0), Vec2::new(10.5, 0.0));
        let edges_only = SpriteSnapRules {
            guides: false,
            ..rules(None, false)
        };
        assert_eq!(
            land(point, BVec2::TRUE, &targets, &edges_only).x,
            Some(11.0)
        );
        let anchored = rules(None, false).without_edges();
        assert_eq!(land(point, BVec2::TRUE, &targets, &anchored).x, None);
        let near_guide = Rect::from_corners(Vec2::new(29.0, 0.0), Vec2::new(29.0, 0.0));
        assert_eq!(
            land(near_guide, BVec2::TRUE, &targets, &anchored).x,
            Some(30.0),
            "a point still lands on a guide"
        );
    }

    #[test]
    fn the_magnet_off_snaps_nothing() {
        let targets = SpriteSnapTargets::from_rects([square(Vec2::new(11.0, 11.0))]);
        let off = SpriteSnapRules {
            magnet: false,
            ..rules(Some(8.0), true)
        };
        let landing = land(square(Vec2::new(0.4, 0.4)), BVec2::TRUE, &targets, &off);
        assert_eq!(landing, SpriteLanding::default());
    }

    #[test]
    fn the_rules_follow_the_canvas_settings() {
        let kinds = CanvasSnap::default();
        let on = SpriteSnapRules::new(&kinds, kinds.magnet(false), 8.0, 4.0);
        assert!(on.magnet);
        assert_eq!(on.grid, Some(8.0));
        assert_eq!(on.pixel, Some(WORLD_UNITS_PER_PIXEL));
        assert_eq!(on.radius, 4.0);
        assert!(on.edges && on.guides);
        assert!(
            !SpriteSnapRules::new(&kinds, kinds.magnet(true), 8.0, 4.0).magnet,
            "the magnet modifier suspends snapping"
        );
        let off = CanvasSnap {
            enabled: false,
            grid: false,
            sprite_edges: false,
            guides: false,
            ..kinds
        };
        let inverted = SpriteSnapRules::new(&off, off.magnet(true), 8.0, 4.0);
        assert!(inverted.magnet, "and turns it on while the master is off");
        assert_eq!(inverted.grid, None);
        assert!(!inverted.edges && !inverted.guides);
        let anchored = on.without_edges();
        assert!(
            !anchored.edges && anchored.guides,
            "an unknown size keeps the guides"
        );
        let hidden = CanvasSnap {
            show_guides: false,
            ..kinds
        };
        assert!(
            !SpriteSnapRules::new(&hidden, true, 8.0, 4.0).guides,
            "a hidden guide is not offered"
        );
    }
}
