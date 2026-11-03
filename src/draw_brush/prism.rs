//! `mesh.add_brush`: a brush extruded from an outline given as numbers, with
//! arcs, a tapered far cap and slices along the depth, checked before anything
//! is spawned so a refused call leaves the scene as it was.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{Brush, PropertyValue};
use serde_json::Value;

use crate::draw_brush::{MIN_EXTRUDE_DEPTH, polygon_self_intersects};
use crate::primitive_ops::{
    MAX_PRIMITIVE_FACES, Placement, add_as_one_entry, float_param, int_param,
};

const MIN_AREA: f32 = 1e-6;
const MERGE_DISTANCE: f32 = 1e-5;
const MAX_ARC_SEGMENTS: i64 = 256;
const MAX_DEPTH_SEGMENTS: i64 = 64;
const MAX_DEPTH: f64 = 10_000.0;
/// A taper below this ends in a point rather than a cap too small to see.
const APEX_TAPER: f32 = 1e-3;
const DEFAULT_ARC_SEGMENTS: usize = 8;

/// The plane an outline is drawn in: `u` and `v` are the world axes its
/// name spells, and the depth runs along the third axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutlinePlane {
    Xy,
    Xz,
    Yz,
}

impl OutlinePlane {
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "xy" => Some(Self::Xy),
            "xz" => Some(Self::Xz),
            "yz" => Some(Self::Yz),
            _ => None,
        }
    }

    /// The world axes `u`, `v` and depth run along.
    fn axes(self) -> (Vec3, Vec3, Vec3) {
        match self {
            Self::Xy => (Vec3::X, Vec3::Y, Vec3::Z),
            Self::Xz => (Vec3::X, Vec3::Z, Vec3::Y),
            Self::Yz => (Vec3::Y, Vec3::Z, Vec3::X),
        }
    }
}

/// What a `mesh.add_brush` call asked for, checked. Kept in the undo history
/// so a redo builds the same brush again.
#[derive(Clone)]
struct PrismRequest {
    brush: Brush,
    placement: Placement,
}

/// The outline's points in `(u, v)`, counter-clockwise, without repeats, and
/// no more than `max_points` of them.
fn read_outline(params: &OperatorParameters, max_points: usize) -> Result<Vec<Vec2>, String> {
    let Some(raw) = params.get("outline") else {
        return Err("`outline` is required: a list of [u, v] points and arc items".into());
    };
    let value = match raw {
        PropertyValue::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| format!("`outline` is not a JSON list: {text}"))?,
        other => return Err(format!("`outline` must be a list, got {other}")),
    };
    let Value::Array(items) = value else {
        return Err("`outline` must be a list of [u, v] points and arc items".into());
    };

    let mut points: Vec<Vec2> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item {
            Value::Array(_) => points.push(point_of(item).ok_or_else(|| {
                format!("`outline` item {index} must be two finite numbers [u, v]")
            })?),
            Value::Object(object) => match object.get("arc") {
                Some(arc) => points.extend(
                    arc_points(arc).map_err(|why| format!("`outline` item {index}: {why}"))?,
                ),
                None => {
                    return Err(format!(
                        "`outline` item {index} must be a [u, v] point or an {{\"arc\": ...}}"
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "`outline` item {index} must be a [u, v] point or an {{\"arc\": ...}}"
                ));
            }
        }
    }

    let mut ring: Vec<Vec2> = Vec::with_capacity(points.len());
    for point in points {
        if ring
            .last()
            .is_none_or(|last| last.distance(point) > MERGE_DISTANCE)
        {
            ring.push(point);
        }
    }
    while ring.len() > 1 && ring[0].distance(ring[ring.len() - 1]) <= MERGE_DISTANCE {
        ring.pop();
    }
    if ring.len() < 3 {
        return Err(format!(
            "`outline` needs at least three distinct points, got {}",
            ring.len()
        ));
    }

    if ring.len() > max_points {
        return Err(format!(
            "`outline` has {} points; with these settings one brush may have {max_points}",
            ring.len()
        ));
    }
    let area = signed_area(&ring);
    if area.abs() < MIN_AREA {
        return Err("`outline` encloses no area".into());
    }
    if polygon_self_intersects(&ring) {
        return Err("`outline` crosses itself".into());
    }
    if area < 0.0 {
        ring.reverse();
    }
    Ok(ring)
}

fn point_of(value: &Value) -> Option<Vec2> {
    let items = value.as_array()?;
    let [u, v] = items.as_slice() else {
        return None;
    };
    let point = Vec2::new(u.as_f64()? as f32, v.as_f64()? as f32);
    point.is_finite().then_some(point)
}

fn number_of(object: &serde_json::Map<String, Value>, key: &str) -> Result<f32, String> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .map(|number| number as f32)
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("arc `{key}` must be a finite number"))
}

/// The points of an `{center, radius, from_deg, to_deg, segments}` arc, both
/// ends included. Angles are degrees counter-clockwise from +u toward +v.
fn arc_points(arc: &Value) -> Result<Vec<Vec2>, String> {
    let Some(object) = arc.as_object() else {
        return Err("`arc` must be an object with center, radius, from_deg and to_deg".into());
    };
    if let Some(key) = object
        .keys()
        .find(|key| !["center", "radius", "from_deg", "to_deg", "segments"].contains(&key.as_str()))
    {
        return Err(format!("an arc takes no `{key}`"));
    }
    let center = object
        .get("center")
        .and_then(point_of)
        .ok_or("arc `center` must be two finite numbers [u, v]")?;
    let radius = number_of(object, "radius")?;
    if radius <= 0.0 {
        return Err(format!("arc `radius` must be above zero, got {radius}"));
    }
    let from = number_of(object, "from_deg")?;
    let to = number_of(object, "to_deg")?;
    let sweep = to - from;
    if sweep == 0.0 || sweep.abs() > 360.0 {
        return Err(format!(
            "an arc must turn by more than 0 and at most 360 degrees, got {sweep}"
        ));
    }
    let segments = match object.get("segments") {
        None => DEFAULT_ARC_SEGMENTS,
        Some(value) => match value.as_i64() {
            Some(count) if (1..=MAX_ARC_SEGMENTS).contains(&count) => count as usize,
            _ => {
                return Err(format!(
                    "arc `segments` must be a whole number from 1 to {MAX_ARC_SEGMENTS}"
                ));
            }
        },
    };
    Ok((0..=segments)
        .map(|step| {
            let degrees = from + sweep * step as f32 / segments as f32;
            center + Vec2::from_angle(degrees.to_radians()) * radius
        })
        .collect())
}

fn signed_area(ring: &[Vec2]) -> f32 {
    let n = ring.len();
    (0..n)
        .map(|i| ring[i].perp_dot(ring[(i + 1) % n]))
        .sum::<f32>()
        / 2.0
}

fn area_centroid(ring: &[Vec2]) -> Vec2 {
    let n = ring.len();
    let mut sum = Vec2::ZERO;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        sum += (a + b) * a.perp_dot(b);
    }
    sum / (6.0 * signed_area(ring))
}

/// The prism `ring` (counter-clockwise in `u, v`) makes, its depth centred
/// on the plane and its far cap scaled by `taper` about the area centroid.
/// `taper` 0 closes the far end to a point.
fn build_prism(
    ring: &[Vec2],
    plane: OutlinePlane,
    depth: f32,
    taper: f32,
    depth_segments: usize,
) -> Result<Brush, String> {
    let (axis_u, axis_v, axis_depth) = plane.axes();
    let n = ring.len();
    let centroid = area_centroid(ring);
    let flipped = axis_u.cross(axis_v).dot(axis_depth) < 0.0;
    let ordered: Vec<Vec2> = if flipped {
        ring.iter().rev().copied().collect()
    } else {
        ring.to_vec()
    };
    let apex = taper == 0.0;
    let rings = if apex {
        depth_segments
    } else {
        depth_segments + 1
    };

    let mut positions: Vec<Vec3> = Vec::with_capacity(rings * n + 1);
    for layer in 0..rings {
        let t = layer as f32 / depth_segments as f32;
        let scale = 1.0 + (taper - 1.0) * t;
        let offset = axis_depth * (depth * (t - 0.5));
        for point in &ordered {
            let scaled = centroid + (*point - centroid) * scale;
            positions.push(axis_u * scaled.x + axis_v * scaled.y + offset);
        }
    }
    let tip = positions.len();
    if apex {
        positions.push(axis_u * centroid.x + axis_v * centroid.y + axis_depth * (depth / 2.0));
    }

    let at = |layer: usize, i: usize| layer * n + i % n;
    let mut polygons: Vec<Vec<usize>> = Vec::with_capacity(2 + n * depth_segments);
    if !apex {
        polygons.push((0..n).map(|i| at(depth_segments, i)).collect());
    }
    polygons.push((0..n).rev().map(|i| at(0, i)).collect());
    for layer in 0..depth_segments {
        for i in 0..n {
            if apex && layer + 1 == depth_segments {
                polygons.push(vec![at(layer, i), at(layer, i + 1), tip]);
            } else {
                polygons.push(vec![
                    at(layer, i),
                    at(layer, i + 1),
                    at(layer + 1, i + 1),
                    at(layer + 1, i),
                ]);
            }
        }
    }
    Brush::from_polygons(positions, polygons).map_err(|why| format!("the outline {why}"))
}

impl PrismRequest {
    fn read(world: &World, params: &OperatorParameters) -> Result<Self, String> {
        const KEYS: [&str; 10] = [
            "outline",
            "plane",
            "depth",
            "taper",
            "depth_segments",
            "pos",
            "rot",
            "material",
            "parent",
            "name",
        ];
        if let Some(key) = params.keys().find(|key| !KEYS.contains(&key.as_str())) {
            return Err(format!("this operator takes no `{key}`"));
        }
        let plane = match params.get("plane") {
            None => OutlinePlane::Xz,
            Some(_) => params
                .as_str("plane")
                .and_then(OutlinePlane::parse)
                .ok_or("`plane` must be xy, xz or yz")?,
        };
        let depth =
            float_param(params, "depth", f64::from(MIN_EXTRUDE_DEPTH)..=MAX_DEPTH)?.unwrap_or(1.0);
        let taper = match float_param(params, "taper", 0.0..=1.0)? {
            Some(taper) if taper < APEX_TAPER => 0.0,
            Some(taper) => taper,
            None => 1.0,
        };
        let depth_segments =
            int_param(params, "depth_segments", 1..=MAX_DEPTH_SEGMENTS)?.unwrap_or(1);
        let placement = Placement::read(world, params)?;

        let caps = if taper == 0.0 { 1 } else { 2 };
        let max_points = MAX_PRIMITIVE_FACES.saturating_sub(caps) / depth_segments;
        let ring = read_outline(params, max_points)?;
        let mut brush = build_prism(&ring, plane, depth, taper, depth_segments)?;
        placement.paint(world, &mut brush);
        Ok(Self { brush, placement })
    }
}

/// Add the brush the outline parameters describe, as one undo entry.
/// Refused parameters add nothing and report `invalid`.
pub(crate) fn add_outline_brush(world: &mut World, params: &OperatorParameters) -> OperatorResult {
    let request = match PrismRequest::read(world, params) {
        Ok(request) => request,
        Err(why) => {
            warn_caller(world, format!("Add Brush: {why}"));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    let placement = request.placement.clone();
    add_as_one_entry(
        world,
        "Add Brush".into(),
        &placement,
        move |world, parent| {
            crate::entity_ops::spawn_brush_under(world, request.brush.clone(), parent, |entity| {
                request.placement.apply(entity, None);
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_turn_arc_has_its_segments_plus_one_points() {
        let arc = serde_json::json!({
            "center": [0.0, 1.0], "radius": 0.5, "from_deg": 0.0, "to_deg": 180.0, "segments": 4
        });
        let points = arc_points(&arc).expect("a valid arc");
        assert_eq!(points.len(), 5);
        assert!(points[0].abs_diff_eq(Vec2::new(0.5, 1.0), 1e-5));
        assert!(points[2].abs_diff_eq(Vec2::new(0.0, 1.5), 1e-5));
        assert!(points[4].abs_diff_eq(Vec2::new(-0.5, 1.0), 1e-5));
    }

    #[test]
    fn a_square_centroid_is_its_middle_either_way_round() {
        let square = [
            Vec2::new(1.0, 1.0),
            Vec2::new(3.0, 1.0),
            Vec2::new(3.0, 3.0),
            Vec2::new(1.0, 3.0),
        ];
        assert!(area_centroid(&square).abs_diff_eq(Vec2::splat(2.0), 1e-5));
        let reversed: Vec<Vec2> = square.iter().rev().copied().collect();
        assert!(area_centroid(&reversed).abs_diff_eq(Vec2::splat(2.0), 1e-5));
    }
}
