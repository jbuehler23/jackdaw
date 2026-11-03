//! A solid made by turning a profile in the (radius, height) half-plane
//! around an axis, as vertex positions and outward-wound face rings.

use glam::{Vec2, Vec3};

use crate::polygon2d::{MERGE_DISTANCE, MIN_AREA, polygon_self_intersects, signed_area};
use crate::{EPSILON, newell_normal};

/// Radii at or below this lie on the axis.
const AXIS_DISTANCE: f32 = 1e-5;
/// Height change below which an edge from the axis is a flat disc.
const FLAT: f32 = 1e-6;

/// The world axis a profile turns around. The profile's height runs along
/// it and the turn starts with the radius along `start`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RevolveAxis {
    X,
    #[default]
    Y,
    Z,
}

impl RevolveAxis {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "z" => Some(Self::Z),
            _ => None,
        }
    }

    /// The axis, the direction the radius starts along, and the direction
    /// it turns toward first (counter-clockwise looking down the axis).
    fn frame(self) -> (Vec3, Vec3, Vec3) {
        let (axis, start) = match self {
            Self::X => (Vec3::X, Vec3::Y),
            Self::Y => (Vec3::Y, Vec3::X),
            Self::Z => (Vec3::Z, Vec3::X),
        };
        (axis, start, axis.cross(start))
    }
}

/// Whether a turn that leaves the solid open may close it with flat faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RevolveCaps {
    /// Close the ends of a partial turn and the off-axis ends of an open
    /// profile with flat faces.
    #[default]
    Auto,
    /// Refuse any profile and angle that would need such a face.
    None,
}

impl RevolveCaps {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RevolveOptions {
    /// Steps over the swept angle, at least 3.
    pub segments: usize,
    /// The swept angle, above 0 and at most 360.
    pub angle_deg: f32,
    pub axis: RevolveAxis,
    pub caps: RevolveCaps,
    /// The most faces the result may have.
    pub max_faces: usize,
}

/// Positions and faces, each ring counter-clockwise seen from outside.
#[derive(Clone, Debug, Default)]
pub struct RevolvedSolid {
    pub positions: Vec<Vec3>,
    pub polygons: Vec<Vec<usize>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RevolveError {
    TooFewPoints(usize),
    NotFinite(usize),
    NegativeRadius(usize),
    TooFewSegments(usize),
    BadAngle(f32),
    NoArea,
    /// The profile crosses itself; `open` when the crossing may come from
    /// closing an open profile along the axis.
    CrossesItself {
        open: bool,
    },
    TouchesAxisTwice,
    PinchesAxis,
    NeedsCaps,
    TooManyFaces {
        faces: usize,
        max: usize,
    },
    TooManyPoints {
        points: usize,
        max: usize,
    },
    /// The swept arc at the widest radius is shorter than the editor's
    /// smallest length.
    SweepTooShort(f32),
    /// A partial turn leaves a gap at the widest radius shorter than the
    /// editor's smallest length.
    GapTooSmall(f32),
    Degenerate,
}

impl std::fmt::Display for RevolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewPoints(count) => {
                write!(f, "needs at least two distinct points, got {count}")
            }
            Self::NotFinite(index) => write!(f, "point {index} is not two finite numbers"),
            Self::NegativeRadius(index) => {
                write!(
                    f,
                    "point {index} has a radius below zero; the profile may not cross the axis"
                )
            }
            Self::TooFewSegments(count) => write!(f, "needs at least 3 segments, got {count}"),
            Self::BadAngle(angle) => {
                write!(
                    f,
                    "the angle must be above 0 and at most 360 degrees, got {angle}"
                )
            }
            Self::NoArea => write!(f, "encloses no area"),
            Self::CrossesItself { open: false } => write!(f, "crosses itself"),
            Self::CrossesItself { open: true } => write!(
                f,
                "crosses itself once its ends are joined to the axis; repeat the first point at \
                 the end to close the profile on itself"
            ),
            Self::TouchesAxisTwice => write!(f, "touches the axis in more than one place"),
            Self::PinchesAxis => write!(
                f,
                "touches the axis at a single point, which a full turn pinches; give it an edge \
                 along the axis"
            ),
            Self::NeedsCaps => write!(
                f,
                "needs flat caps to close (a partial turn, or an open end off the axis), but caps \
                 are none"
            ),
            Self::TooManyFaces { faces, max } => {
                write!(
                    f,
                    "makes {faces} faces, more than the {max} one brush may have"
                )
            }
            Self::TooManyPoints { points, max } => write!(
                f,
                "has {points} points; with these segments one brush may have {max}"
            ),
            Self::SweepTooShort(arc) => write!(
                f,
                "sweeps an arc of only {arc} at its widest radius; turn it further"
            ),
            Self::GapTooSmall(gap) => write!(
                f,
                "leaves a gap of only {gap} at its widest radius; use 360 degrees for a full turn"
            ),
            Self::Degenerate => write!(f, "makes a face with no area"),
        }
    }
}

impl std::error::Error for RevolveError {}

/// The closed ring a profile bounds, counter-clockwise in (radius, height),
/// with every run along the axis reduced to its two ends.
struct ProfileRing {
    points: Vec<Vec2>,
    /// An open end off the axis was closed with a disc.
    open_ends: bool,
    /// Separate stretches of the ring that lie on the axis.
    axis_runs: usize,
}

fn profile_ring(profile: &[Vec2], max_points: usize) -> Result<ProfileRing, RevolveError> {
    let mut points: Vec<Vec2> = Vec::with_capacity(profile.len() + 2);
    for (index, point) in profile.iter().enumerate() {
        if !point.is_finite() {
            return Err(RevolveError::NotFinite(index));
        }
        if point.x < -AXIS_DISTANCE {
            return Err(RevolveError::NegativeRadius(index));
        }
        let point = Vec2::new(
            if point.x <= AXIS_DISTANCE {
                0.0
            } else {
                point.x
            },
            point.y,
        );
        if points
            .last()
            .is_none_or(|last| last.distance(point) > MERGE_DISTANCE)
        {
            points.push(point);
        }
    }
    let closed = points.len() > 2 && points[0].distance(points[points.len() - 1]) <= MERGE_DISTANCE;
    if closed {
        points.pop();
    }
    if points.len() < 2 {
        return Err(RevolveError::TooFewPoints(points.len()));
    }

    let mut open_ends = false;
    if !closed {
        let (first, last) = (points[0], points[points.len() - 1]);
        if last.x > 0.0 {
            points.push(Vec2::new(0.0, last.y));
            open_ends = true;
        }
        if first.x > 0.0 {
            points.insert(0, Vec2::new(0.0, first.y));
            open_ends = true;
        }
        while points.len() > 1 && points[0].distance(points[points.len() - 1]) <= MERGE_DISTANCE {
            points.pop();
        }
    }
    if points.len() < 3 {
        return Err(RevolveError::NoArea);
    }
    if points.len() > max_points {
        return Err(RevolveError::TooManyPoints {
            points: points.len(),
            max: max_points,
        });
    }
    if polygon_self_intersects(&points) {
        return Err(RevolveError::CrossesItself { open: !closed });
    }
    let area = signed_area(&points);
    if area.abs() < MIN_AREA {
        return Err(RevolveError::NoArea);
    }
    if area < 0.0 {
        points.reverse();
    }

    let n = points.len();
    let on_axis = |i: usize| points[i % n].x == 0.0;
    let axis_runs = (0..n)
        .filter(|&i| on_axis(i) && !on_axis(i + n - 1))
        .count();
    let points = (0..n)
        .filter(|&i| !(on_axis(i) && on_axis(i + n - 1) && on_axis(i + 1)))
        .map(|i| points[i])
        .collect();
    Ok(ProfileRing {
        points,
        open_ends,
        axis_runs,
    })
}

/// What one profile edge sweeps into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sweep {
    /// Along the axis: nothing.
    Axis,
    /// From the axis at one height: one flat face.
    Disc,
    /// A band of `segments` quads, or triangles where it meets the axis.
    Band,
}

fn sweep_of(a: Vec2, b: Vec2) -> Sweep {
    match (a.x == 0.0, b.x == 0.0) {
        (true, true) => Sweep::Axis,
        (true, false) | (false, true) if (b.y - a.y).abs() <= FLAT => Sweep::Disc,
        _ => Sweep::Band,
    }
}

/// Turn `profile`, a list of `[radius, height]` points, around `options.axis`.
///
/// A profile whose last point repeats its first is closed on itself (a torus
/// when no point is on the axis); otherwise its ends are joined to the axis,
/// with flat discs where they are off it. Points on the axis become poles. A
/// partial turn is closed with the profile as a flat face at each end.
pub fn revolve_profile(
    profile: &[Vec2],
    options: &RevolveOptions,
) -> Result<RevolvedSolid, RevolveError> {
    let segments = options.segments;
    if segments < 3 {
        return Err(RevolveError::TooFewSegments(segments));
    }
    let angle = options.angle_deg;
    if !angle.is_finite() || angle <= 0.0 || angle > 360.0 {
        return Err(RevolveError::BadAngle(angle));
    }
    let full = angle == 360.0;
    // Every edge but the two discs and the axis closing an open profile
    // sweeps a band of `segments` faces.
    let max_points = options.max_faces / segments + 3;
    let ProfileRing {
        points: ring,
        open_ends,
        axis_runs,
    } = profile_ring(profile, max_points)?;
    let n = ring.len();
    if full && axis_runs > 1 {
        return Err(RevolveError::TouchesAxisTwice);
    }
    let axis_points = ring.iter().filter(|point| point.x == 0.0).count();
    if full && axis_points == 1 {
        return Err(RevolveError::PinchesAxis);
    }
    if (open_ends || !full) && options.caps == RevolveCaps::None {
        return Err(RevolveError::NeedsCaps);
    }
    if !full {
        let widest = ring.iter().map(|point| point.x).fold(0.0, f32::max);
        let arc = widest * angle.to_radians();
        if arc < EPSILON {
            return Err(RevolveError::SweepTooShort(arc));
        }
        let gap = widest * (360.0 - angle).to_radians();
        if gap < EPSILON {
            return Err(RevolveError::GapTooSmall(gap));
        }
    }

    let edges: Vec<Sweep> = (0..n)
        .map(|i| sweep_of(ring[i], ring[(i + 1) % n]))
        .collect();
    let faces = edges
        .iter()
        .map(|sweep| match sweep {
            Sweep::Axis => 0,
            Sweep::Disc => 1,
            Sweep::Band => segments,
        })
        .sum::<usize>()
        + if full { 0 } else { 2 };
    if faces > options.max_faces {
        return Err(RevolveError::TooManyFaces {
            faces,
            max: options.max_faces,
        });
    }

    let (axis, start, toward) = options.axis.frame();
    let step = angle.to_radians() / segments as f32;
    let radial = |theta: f32| start * theta.cos() + toward * theta.sin();
    let columns = if full { segments } else { segments + 1 };

    let mut positions: Vec<Vec3> = Vec::new();
    let mut first: Vec<usize> = Vec::with_capacity(n);
    for point in &ring {
        first.push(positions.len());
        if point.x == 0.0 {
            positions.push(axis * point.y);
        } else {
            for column in 0..columns {
                positions.push(radial(step * column as f32) * point.x + axis * point.y);
            }
        }
    }
    let at = |i: usize, column: usize| {
        if ring[i].x == 0.0 {
            first[i]
        } else if full {
            first[i] + column % segments
        } else {
            first[i] + column
        }
    };
    let outward = |a: Vec2, b: Vec2, theta: f32| {
        let d = b - a;
        radial(theta) * d.y - axis * d.x
    };

    let mut polygons: Vec<(Vec<usize>, Vec3)> = Vec::with_capacity(faces);
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b) = (ring[i], ring[j]);
        match edges[i] {
            Sweep::Axis => {}
            Sweep::Disc => {
                let (rim, hub) = if a.x == 0.0 { (j, i) } else { (i, j) };
                let mut corners: Vec<usize> = (0..columns).map(|column| at(rim, column)).collect();
                if !full {
                    corners.push(at(hub, 0));
                }
                polygons.push((corners, outward(a, b, angle.to_radians() / 2.0)));
            }
            Sweep::Band => {
                for column in 0..segments {
                    let mut corners = vec![
                        at(i, column),
                        at(j, column),
                        at(j, column + 1),
                        at(i, column + 1),
                    ];
                    corners.dedup();
                    if corners.first() == corners.last() {
                        corners.pop();
                    }
                    polygons.push((corners, outward(a, b, step * (column as f32 + 0.5))));
                }
            }
        }
    }
    if !full {
        let end = angle.to_radians();
        polygons.push(((0..n).map(|i| at(i, 0)).collect(), -toward));
        polygons.push((
            (0..n).map(|i| at(i, segments)).collect(),
            -start * end.sin() + toward * end.cos(),
        ));
    }

    let polygons = polygons
        .into_iter()
        .map(|(mut corners, expected)| {
            let ring: Vec<Vec3> = corners.iter().map(|&index| positions[index]).collect();
            let normal = newell_normal(&ring);
            if normal == Vec3::ZERO {
                return Err(RevolveError::Degenerate);
            }
            if normal.dot(expected) < 0.0 {
                corners.reverse();
            }
            Ok(corners)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(compact(positions, polygons))
}

/// Drop positions no face uses (a pole hidden under a flat disc).
fn compact(positions: Vec<Vec3>, polygons: Vec<Vec<usize>>) -> RevolvedSolid {
    let mut remap = vec![usize::MAX; positions.len()];
    let mut kept = Vec::with_capacity(positions.len());
    let polygons = polygons
        .into_iter()
        .map(|corners| {
            corners
                .into_iter()
                .map(|index| {
                    if remap[index] == usize::MAX {
                        remap[index] = kept.len();
                        kept.push(positions[index]);
                    }
                    remap[index]
                })
                .collect()
        })
        .collect();
    RevolvedSolid {
        positions: kept,
        polygons,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn options(segments: usize, angle_deg: f32) -> RevolveOptions {
        RevolveOptions {
            segments,
            angle_deg,
            axis: RevolveAxis::Y,
            caps: RevolveCaps::Auto,
            max_faces: 4096,
        }
    }

    fn circle(center: Vec2, radius: f32, points: usize) -> Vec<Vec2> {
        let mut ring: Vec<Vec2> = (0..points)
            .map(|i| {
                center + Vec2::from_angle(i as f32 / points as f32 * std::f32::consts::TAU) * radius
            })
            .collect();
        ring.push(ring[0]);
        ring
    }

    /// Every directed edge appears once and its reverse once: closed and
    /// consistently wound. Returns V - E + F.
    fn euler_of_closed(solid: &RevolvedSolid) -> i64 {
        let mut directed: HashMap<(usize, usize), usize> = HashMap::new();
        for ring in &solid.polygons {
            for i in 0..ring.len() {
                *directed
                    .entry((ring[i], ring[(i + 1) % ring.len()]))
                    .or_default() += 1;
            }
        }
        for (&(a, b), &count) in &directed {
            assert_eq!(count, 1, "edge {a}->{b} used {count} times");
            assert_eq!(directed.get(&(b, a)), Some(&1), "edge {a}->{b} has no twin");
        }
        solid.positions.len() as i64 - (directed.len() / 2) as i64 + solid.polygons.len() as i64
    }

    fn volume(solid: &RevolvedSolid) -> f32 {
        let mut total = 0.0;
        for ring in &solid.polygons {
            let p: Vec<Vec3> = ring.iter().map(|&i| solid.positions[i]).collect();
            for i in 1..p.len() - 1 {
                total += p[0].dot(p[i].cross(p[i + 1])) / 6.0;
            }
        }
        total
    }

    fn bounds(solid: &RevolvedSolid) -> (Vec3, Vec3) {
        solid
            .positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(min, max), p| {
                (min.min(*p), max.max(*p))
            })
    }

    #[test]
    fn an_open_profile_off_the_axis_gets_a_disc_at_each_end() {
        let barrel = [
            Vec2::new(0.4, 0.0),
            Vec2::new(0.47, 0.25),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.47, 0.75),
            Vec2::new(0.4, 1.0),
        ];
        let solid = revolve_profile(&barrel, &options(12, 360.0)).expect("a barrel");
        assert_eq!(solid.polygons.len(), 12 * 4 + 2);
        assert_eq!(solid.positions.len(), 12 * 5);
        assert_eq!(euler_of_closed(&solid), 2);
        assert!(volume(&solid) > 0.0);
        let (min, max) = bounds(&solid);
        assert!(min.abs_diff_eq(Vec3::new(-0.5, 0.0, -0.5), 1e-4), "{min}");
        assert!(max.abs_diff_eq(Vec3::new(0.5, 1.0, 0.5), 1e-4), "{max}");
    }

    #[test]
    fn a_closed_profile_off_the_axis_is_a_watertight_torus() {
        let solid = revolve_profile(&circle(Vec2::new(1.0, 0.0), 0.2, 8), &options(12, 360.0))
            .expect("a torus");
        assert_eq!(solid.polygons.len(), 12 * 8);
        assert_eq!(euler_of_closed(&solid), 0);
        assert!(volume(&solid) > 0.0);
    }

    #[test]
    fn points_on_the_axis_close_as_poles_with_triangle_fans() {
        let cone = [
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        let solid = revolve_profile(&cone, &options(8, 360.0)).expect("a cone");
        // The slope fans to the tip; the base is one disc and its pole is unused.
        assert_eq!(solid.polygons.len(), 8 + 1);
        assert_eq!(solid.positions.len(), 8 + 1);
        assert_eq!(euler_of_closed(&solid), 2);
        let expected = 4.0 * std::f32::consts::FRAC_PI_4.sin() / 3.0;
        assert!(
            (volume(&solid) - expected).abs() < 1e-4,
            "{}",
            volume(&solid)
        );
    }

    #[test]
    fn a_half_turn_has_two_flat_caps_in_the_start_and_end_planes() {
        let ring = [
            Vec2::new(0.8, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.8, 1.0),
            Vec2::new(0.8, 0.0),
        ];
        let solid = revolve_profile(&ring, &options(6, 180.0)).expect("half a ring");
        assert_eq!(solid.polygons.len(), 6 * 4 + 2);
        assert_eq!(euler_of_closed(&solid), 2);
        let flat_on = |normal: Vec3| {
            solid
                .polygons
                .iter()
                .filter(|ring| {
                    let p: Vec<Vec3> = ring.iter().map(|&i| solid.positions[i]).collect();
                    newell_normal(&p).abs_diff_eq(normal, 1e-4)
                        && p.iter().all(|q| q.dot(normal).abs() < 1e-4)
                })
                .count()
        };
        // Starts along +X, turns toward -Z, ends along -X: both caps lie in z = 0.
        assert_eq!(flat_on(Vec3::Z), 2, "two planar caps in z = 0 facing +z");
        assert!(volume(&solid) > 0.0);
    }

    #[test]
    fn each_axis_names_the_direction_the_height_runs_along() {
        let rod = [Vec2::new(0.5, 0.0), Vec2::new(0.5, 2.0)];
        for (axis, extent) in [
            (RevolveAxis::X, Vec3::new(2.0, 1.0, 1.0)),
            (RevolveAxis::Y, Vec3::new(1.0, 2.0, 1.0)),
            (RevolveAxis::Z, Vec3::new(1.0, 1.0, 2.0)),
        ] {
            let solid = revolve_profile(
                &rod,
                &RevolveOptions {
                    axis,
                    ..options(4, 360.0)
                },
            )
            .expect("a rod");
            let (min, max) = bounds(&solid);
            assert!(
                (max - min).abs_diff_eq(extent, 1e-4),
                "{axis:?}: {min} {max}"
            );
            assert!(volume(&solid) > 0.0, "{axis:?}");
            assert_eq!(euler_of_closed(&solid), 2);
        }
    }

    #[test]
    fn bad_profiles_and_counts_are_refused() {
        let rod = [Vec2::new(0.5, 0.0), Vec2::new(0.5, 1.0)];
        let refused = |profile: &[Vec2], options: RevolveOptions| {
            revolve_profile(profile, &options).unwrap_err()
        };
        assert_eq!(
            refused(&rod[..1], options(8, 360.0)),
            RevolveError::TooFewPoints(1)
        );
        assert_eq!(
            refused(
                &[Vec2::new(-0.1, 0.0), Vec2::new(0.5, 1.0)],
                options(8, 360.0)
            ),
            RevolveError::NegativeRadius(0)
        );
        assert_eq!(
            refused(&rod, options(2, 360.0)),
            RevolveError::TooFewSegments(2)
        );
        assert_eq!(refused(&rod, options(8, 0.0)), RevolveError::BadAngle(0.0));
        assert_eq!(
            refused(&rod, options(8, 361.0)),
            RevolveError::BadAngle(361.0)
        );
        assert_eq!(
            refused(
                &[Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0)],
                options(8, 360.0)
            ),
            RevolveError::NoArea
        );
        assert!(matches!(
            refused(&circle(Vec2::new(1.0, 0.0), 0.2, 8)[..8], options(8, 360.0)),
            RevolveError::CrossesItself { open: true }
        ));
        let bowtie = [
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
        ];
        assert_eq!(
            refused(&bowtie, options(8, 360.0)),
            RevolveError::CrossesItself { open: false }
        );
        let pinch = [
            Vec2::new(0.0, 0.5),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 0.5),
        ];
        assert_eq!(
            refused(&pinch, options(8, 360.0)),
            RevolveError::PinchesAxis
        );
        assert!(
            revolve_profile(&pinch, &options(8, 90.0)).is_ok(),
            "a partial turn does not pinch"
        );
        let dart = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
            Vec2::new(0.5, 1.0),
            Vec2::new(0.0, 0.0),
        ];
        assert_eq!(
            refused(&dart, options(8, 360.0)),
            RevolveError::TouchesAxisTwice
        );
        let half_dart = revolve_profile(&dart, &options(8, 180.0)).expect("a partial turn");
        assert_eq!(euler_of_closed(&half_dart), 2);
        assert!(volume(&half_dart) > 0.0);
        let none = RevolveOptions {
            caps: RevolveCaps::None,
            ..options(8, 360.0)
        };
        assert_eq!(refused(&rod, none), RevolveError::NeedsCaps);
        assert_eq!(
            refused(
                &circle(Vec2::new(1.0, 0.0), 0.2, 8),
                RevolveOptions {
                    caps: RevolveCaps::None,
                    ..options(8, 180.0)
                }
            ),
            RevolveError::NeedsCaps
        );
        assert!(revolve_profile(&circle(Vec2::new(1.0, 0.0), 0.2, 8), &none).is_ok());
        assert_eq!(
            refused(
                &rod,
                RevolveOptions {
                    max_faces: 10,
                    ..options(16, 360.0)
                }
            ),
            RevolveError::TooManyPoints { points: 4, max: 3 }
        );
        assert_eq!(
            refused(
                &rod,
                RevolveOptions {
                    max_faces: 17,
                    ..options(16, 360.0)
                }
            ),
            RevolveError::TooManyFaces { faces: 18, max: 17 }
        );
        assert!(matches!(
            refused(&rod, options(8, 0.01)),
            RevolveError::SweepTooShort(_)
        ));
        assert!(matches!(
            refused(&rod, options(8, 359.99)),
            RevolveError::GapTooSmall(_)
        ));
        assert!(matches!(
            refused(
                &bowtie,
                RevolveOptions {
                    caps: RevolveCaps::None,
                    ..options(8, 90.0)
                }
            ),
            RevolveError::CrossesItself { .. }
        ));
    }

    #[test]
    fn a_three_millimetre_peg_turns_like_a_large_one() {
        let peg = [Vec2::new(0.0015, 0.0), Vec2::new(0.0015, 0.003)];
        let solid = revolve_profile(&peg, &options(8, 360.0)).expect("a small peg");
        assert_eq!(solid.polygons.len(), 8 + 2);
        assert_eq!(euler_of_closed(&solid), 2);
        assert!(volume(&solid) > 0.0);
    }
}
