//! `brush.elements.transform`: move, rotate and scale the selected faces,
//! edges or vertices of every brush being edited by value, with an optional
//! soft falloff that carries nearby vertices part of the way.

use bevy::math::Affine3A;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{MeshMirror, ModifierStack};
use jackdaw_scene_types::{Brush, BrushTopology};

use super::param_edit;
use super::select_elements::refuse;
use jackdaw_editor_core::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};
use jackdaw_editor_core::brush_drag_ops::apply_vertex_deltas;
use jackdaw_editor_core::numeric_transform::{centroid, sub_selection_vertices};
use jackdaw_editor_core::vec3_param;

const LABEL: &str = "Transform Elements";

/// Least area a face the transform reshapes may be left with.
const MIN_FACE_AREA: f32 = 1e-8;

/// Least distance between two vertices of a face the transform reshapes.
const MIN_VERTEX_GAP: f32 = 1e-6;

/// Least move, relative to the size of the coordinates involved, that counts
/// as moving a vertex rather than rounding.
pub(super) const MOVE_TOLERANCE: f32 = 1e-6;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushElementsTransformOp>();
}

#[operator(
    id = "brush.elements.transform",
    label = "Transform Elements",
    description = "Move, rotate and scale the selected faces, edges or vertices of every brush \
                   being edited by value: scale, then rotate, about the pivot, then translate.",
    is_available = can_transform_elements,
    allows_undo = true,
    params(
        translate(Vec3, doc = "Offset [x, y, z]. Defaults to none."),
        rotate(
            Vec3,
            doc = "Rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        scale(
            Vec3,
            doc = "Factor [x, y, z]; 0 flattens onto the pivot along that axis. Defaults to 1."
        ),
        pivot(
            String,
            doc = "centroid (of every selected vertex), origin (of each brush) or point. \
                   Defaults to centroid, or point when `point` is given."
        ),
        point(Vec3, doc = "Pivot for `pivot` point, in `space`."),
        space(
            String,
            doc = "world or local (each brush's own axes). Defaults to world."
        ),
        falloff_radius(
            f64,
            doc = "Carry unselected vertices within this distance of the selection part of \
                   the way, in `space` units. Defaults to none."
        ),
        falloff_curve(
            String,
            doc = "How the falloff fades with distance: smooth, linear, sharp or root. \
                   Defaults to smooth."
        ),
    )
)]
pub(crate) fn brush_elements_transform(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let transform = match ElementTransform::read(&params) {
        Ok(transform) => transform,
        Err(why) => return refuse(world, LABEL, why),
    };
    if !is_element_mode(*world.resource::<EditMode>()) {
        return unchanged(world, LABEL, "no faces, edges or vertices are being edited");
    }
    let targets = targets(world);
    let picked_world: Vec<Vec3> = targets
        .iter()
        .flat_map(|target| {
            target
                .picked
                .iter()
                .zip(&target.brush.topology.vertices)
                .filter(|(picked, _)| **picked)
                .map(|(_, vert)| target.to_world.transform_point3(vert.position))
        })
        .collect();
    if picked_world.is_empty() {
        return unchanged(world, LABEL, "nothing is selected to transform");
    }
    let pivot_world = centroid(&picked_world);

    let mut edits = Vec::new();
    for target in &targets {
        match transform.reshape(target, pivot_world) {
            Ok(Some(edit)) => edits.push(edit),
            Ok(None) => {}
            Err(why) => return refuse(world, LABEL, why),
        }
    }
    if edits.is_empty() {
        return unchanged(
            world,
            LABEL,
            "the transform leaves every vertex where it is",
        );
    }
    commit(world, LABEL, edits)
}

/// Put every reshaped brush in place and report how many vertices moved.
pub(super) fn commit(world: &mut World, label: &str, edits: Vec<Reshaped>) -> OperatorResult {
    let moved: usize = edits.iter().map(|edit| edit.moved).sum();
    let brushes = edits.len();
    for edit in edits {
        if let Some(mut live) = world.get_mut::<Brush>(edit.entity) {
            *live = edit.brush;
        }
        world.entity_mut(edit.entity).insert(edit.halfedge);
    }
    let report = if brushes == 1 {
        format!("{label}: moved {moved} vertices")
    } else {
        format!("{label}: moved {moved} vertices on {brushes} brushes")
    };
    report_to_caller(world, report);
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}

pub(crate) fn can_transform_elements(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    is_element_mode(*edit_mode)
        && selection
            .brushes
            .values()
            .any(|sub| !sub.faces.is_empty() || !sub.edges.is_empty() || !sub.vertices.is_empty())
}

/// Whether faces, edges or vertices are being edited.
pub(super) fn is_element_mode(edit_mode: EditMode) -> bool {
    matches!(
        edit_mode,
        EditMode::BrushEdit(BrushEditMode::Face | BrushEditMode::Edge | BrushEditMode::Vertex)
    )
}

pub(super) fn unchanged(world: &mut World, label: &str, why: &str) -> OperatorResult {
    warn_caller(world, format!("{label}: {why}"));
    set_outcome(world, Outcome::Unchanged);
    OperatorResult::Cancelled
}

/// A brush to reshape, as the operator found it.
pub(super) struct Target {
    pub(super) entity: Entity,
    pub(super) brush: Brush,
    pub(super) rings: Vec<Vec<usize>>,
    /// Per vertex, whether the operator acts on it.
    pub(super) picked: Vec<bool>,
    pub(super) to_world: Affine3A,
    pub(super) mirror: Option<MeshMirror>,
}

impl Target {
    /// `entity`'s brush with the vertices `pick` chooses from its face
    /// rings; `None` when it is no brush or `pick` chooses nothing.
    pub(super) fn new(
        world: &World,
        entity: Entity,
        pick: impl FnOnce(&Brush, &[Vec<usize>]) -> Vec<usize>,
    ) -> Option<Self> {
        let brush = world.get::<Brush>(entity)?.clone();
        let rings = face_rings(&brush.topology);
        let mut picked = vec![false; brush.topology.vertices.len()];
        for vert in pick(&brush, &rings) {
            if let Some(slot) = picked.get_mut(vert) {
                *slot = true;
            }
        }
        if !picked.contains(&true) {
            return None;
        }
        let to_world = jackdaw_scene_types::unsorted_global(world, entity)
            .map_or(Affine3A::IDENTITY, |global| global.affine());
        let mirror = world
            .get::<ModifierStack>(entity)
            .and_then(ModifierStack::first_enabled_mirror)
            .cloned();
        Some(Self {
            entity,
            brush,
            rings,
            picked,
            to_world,
            mirror,
        })
    }

    pub(super) fn positions(&self) -> Vec<Vec3> {
        self.brush
            .topology
            .vertices
            .iter()
            .map(|vert| vert.position)
            .collect()
    }

    /// The brush with the vertices at `indices` moved to `positions`
    /// (brush-local), clipped at an enabled mirror plane, and refused when a
    /// face it reshapes is left with no area or two vertices in one place.
    /// `None` when `indices` is empty.
    pub(super) fn reshape(
        &self,
        before: &[Vec3],
        indices: &[usize],
        positions: &[Vec3],
    ) -> Result<Option<Reshaped>, String> {
        if indices.is_empty() {
            return Ok(None);
        }
        let mut brush = self.brush.clone();
        let mut halfedge = BrushHalfedge::from_topology(&brush.topology);
        apply_vertex_deltas(
            &mut brush,
            Some(&mut halfedge),
            self.mirror.as_ref(),
            &self.brush,
            before,
            &self.rings,
            indices,
            positions,
        );
        let mut moved = vec![false; before.len()];
        for &vert in indices {
            moved[vert] = true;
        }
        if let Some(why) = degenerate_face(self.entity, &brush, &self.rings, &moved) {
            return Err(why);
        }
        Ok(Some(Reshaped {
            entity: self.entity,
            brush,
            halfedge,
            moved: indices.len(),
        }))
    }
}

/// One brush reshaped, ready to replace the live one.
pub(super) struct Reshaped {
    entity: Entity,
    pub(super) brush: Brush,
    halfedge: BrushHalfedge,
    moved: usize,
}

/// Every brush being edited whose selection touches a vertex.
pub(super) fn targets(world: &World) -> Vec<Target> {
    let selection = world.resource::<BrushSelection>();
    let mut entities: Vec<Entity> = selection.edit_brushes().collect();
    entities.sort();
    entities
        .into_iter()
        .filter_map(|entity| {
            let sub = selection.sub(entity)?;
            Target::new(world, entity, |_, rings| sub_selection_vertices(sub, rings))
        })
        .collect()
}

fn face_rings(topology: &BrushTopology) -> Vec<Vec<usize>> {
    (0..topology.polygons.len())
        .map(|face| topology.face_ring(face).map(|vert| vert as usize).collect())
        .collect()
}

/// Why `brush` cannot keep its new shape: the first face touching a `moved`
/// vertex that has no area or two vertices in one place.
fn degenerate_face(
    entity: Entity,
    brush: &Brush,
    rings: &[Vec<usize>],
    moved: &[bool],
) -> Option<String> {
    let positions: Vec<Vec3> = brush
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position)
        .collect();
    rings.iter().enumerate().find_map(|(face, ring)| {
        if !ring
            .iter()
            .any(|&vert| moved.get(vert).copied().unwrap_or(false))
        {
            return None;
        }
        let points: Vec<Vec3> = ring.iter().map(|&vert| positions[vert]).collect();
        if ring_area(&points) <= MIN_FACE_AREA {
            return Some(format!(
                "face {face} of brush {entity} would be left with no area"
            ));
        }
        let doubled = points.iter().enumerate().any(|(index, a)| {
            points[index + 1..]
                .iter()
                .any(|b| a.distance(*b) <= MIN_VERTEX_GAP)
        });
        doubled
            .then(|| format!("face {face} of brush {entity} would have two vertices in one place"))
    })
}

/// Area of a polygon ring, projected onto its best-fit plane.
fn ring_area(ring: &[Vec3]) -> f32 {
    let twice: Vec3 = ring
        .iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a.cross(*b))
        .sum();
    0.5 * twice.length()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pivot {
    Centroid,
    Origin,
    Point(Vec3),
}

/// The axes an operator's offsets are given along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Space {
    World,
    Local,
}

impl Space {
    /// The `space` parameter: world or local, world when absent.
    pub(super) fn read(params: &OperatorParameters) -> Result<Self, String> {
        match text(params, "space")? {
            None | Some("world") => Ok(Self::World),
            Some("local") => Ok(Self::Local),
            Some(other) => Err(format!("`space` {other:?} must be world or local")),
        }
    }

    /// The affine from brush-local coordinates into this space.
    pub(super) fn local_to_space(self, to_world: Affine3A) -> Affine3A {
        match self {
            Self::World => to_world,
            Self::Local => Affine3A::IDENTITY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FalloffCurve {
    Smooth,
    Linear,
    Sharp,
    Root,
}

impl FalloffCurve {
    /// The share of the transform at `t`, 1 at the selection and 0 at the
    /// radius.
    fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Smooth => t * t * (3.0 - 2.0 * t),
            Self::Linear => t,
            Self::Sharp => t * t,
            Self::Root => t.sqrt(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Falloff {
    radius: f32,
    curve: FalloffCurve,
}

/// A `brush.elements.transform` call, checked.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ElementTransform {
    translate: Vec3,
    rotate: Quat,
    scale: Vec3,
    pivot: Pivot,
    space: Space,
    falloff: Option<Falloff>,
}

impl ElementTransform {
    fn read(params: &OperatorParameters) -> Result<Self, String> {
        let translate = vec3_param(params, "translate")?;
        let rotate = vec3_param(params, "rotate")?;
        let scale = vec3_param(params, "scale")?;
        if translate.is_none() && rotate.is_none() && scale.is_none() {
            return Err("give `translate`, `rotate` or `scale`".into());
        }
        if let Some(scale) = scale
            && scale.x * scale.y * scale.z < 0.0
        {
            return Err(format!(
                "`scale` {scale} is negative on an odd number of axes, which would turn the \
                 faces inside out"
            ));
        }
        let point = vec3_param(params, "point")?;
        let pivot = match text(params, "pivot")? {
            None => point.map_or(Pivot::Centroid, Pivot::Point),
            Some("point") => Pivot::Point(point.ok_or("`pivot` point needs a `point`")?),
            Some(named @ ("centroid" | "origin")) => {
                if point.is_some() {
                    return Err("give `point` only with `pivot` point".into());
                }
                if named == "origin" {
                    Pivot::Origin
                } else {
                    Pivot::Centroid
                }
            }
            Some(other) => {
                return Err(format!(
                    "`pivot` {other:?} must be centroid, origin or point"
                ));
            }
        };
        let space = Space::read(params)?;
        let curve = match text(params, "falloff_curve")? {
            None | Some("smooth") => FalloffCurve::Smooth,
            Some("linear") => FalloffCurve::Linear,
            Some("sharp") => FalloffCurve::Sharp,
            Some("root") => FalloffCurve::Root,
            Some(other) => {
                return Err(format!(
                    "`falloff_curve` {other:?} must be smooth, linear, sharp or root"
                ));
            }
        };
        let falloff = match param_edit::number(params, "falloff_radius")? {
            None if params.contains_key("falloff_curve") => {
                return Err("`falloff_curve` needs a `falloff_radius`".into());
            }
            None => None,
            Some(radius) if radius <= 0.0 => {
                return Err(format!("`falloff_radius` {radius} must be above zero"));
            }
            Some(radius) => Some(Falloff { radius, curve }),
        };
        let degrees = rotate.unwrap_or_default();
        Ok(Self {
            translate: translate.unwrap_or_default(),
            rotate: Quat::from_euler(
                EulerRot::YXZ,
                degrees.x.to_radians(),
                degrees.y.to_radians(),
                degrees.z.to_radians(),
            ),
            scale: scale.unwrap_or(Vec3::ONE),
            pivot,
            space,
            falloff,
        })
    }

    /// `target` reshaped, with `pivot_world` the centroid of every selected
    /// vertex. `None` when no vertex moves.
    fn reshape(&self, target: &Target, pivot_world: Vec3) -> Result<Option<Reshaped>, String> {
        let before = target.positions();
        let (indices, positions) =
            self.moves(&before, &target.picked, target.to_world, pivot_world);
        target.reshape(&before, &indices, &positions)
    }

    /// The vertices of `positions` (brush-local) that move and where to,
    /// brush-local. `to_world` places the brush in the world.
    fn moves(
        &self,
        positions: &[Vec3],
        picked: &[bool],
        to_world: Affine3A,
        pivot_world: Vec3,
    ) -> (Vec<usize>, Vec<Vec3>) {
        let to_space = self.space.local_to_space(to_world);
        let from_space = to_space.inverse();
        let in_space: Vec<Vec3> = positions
            .iter()
            .map(|&point| to_space.transform_point3(point))
            .collect();
        let picked_in_space: Vec<Vec3> = in_space
            .iter()
            .zip(picked)
            .filter(|(_, picked)| **picked)
            .map(|(point, _)| *point)
            .collect();
        let pivot = match self.pivot {
            Pivot::Centroid => {
                to_space.transform_point3(to_world.inverse().transform_point3(pivot_world))
            }
            Pivot::Origin => to_space.transform_point3(Vec3::ZERO),
            Pivot::Point(point) => point,
        };
        let (axis, angle) = self.rotate.to_axis_angle();
        let mut indices = Vec::new();
        let mut moved = Vec::new();
        for (vert, &point) in in_space.iter().enumerate() {
            let weight = if picked[vert] {
                1.0
            } else {
                self.falloff_weight(point, &picked_in_space)
            };
            if weight <= 0.0 {
                continue;
            }
            let scale = Vec3::ONE + (self.scale - Vec3::ONE) * weight;
            let rotate = if weight >= 1.0 {
                self.rotate
            } else {
                Quat::from_axis_angle(axis, angle * weight)
            };
            let delta =
                pivot + rotate * (scale * (point - pivot)) + self.translate * weight - point;
            let size = 1.0 + point.abs().max_element().max(pivot.abs().max_element());
            if delta.length() <= MOVE_TOLERANCE * size {
                continue;
            }
            indices.push(vert);
            moved.push(positions[vert] + from_space.transform_vector3(delta));
        }
        (indices, moved)
    }

    /// How much of the transform an unpicked vertex at `point` takes.
    fn falloff_weight(&self, point: Vec3, picked: &[Vec3]) -> f32 {
        let Some(falloff) = self.falloff else {
            return 0.0;
        };
        let nearest = picked
            .iter()
            .map(|&other| other.distance(point))
            .fold(f32::INFINITY, f32::min);
        if nearest >= falloff.radius {
            return 0.0;
        }
        falloff.curve.weight(1.0 - nearest / falloff.radius)
    }
}

/// A text parameter, `None` when absent.
pub(super) fn text<'a>(
    params: &'a OperatorParameters,
    key: &str,
) -> Result<Option<&'a str>, String> {
    if !params.contains_key(key) {
        return Ok(None);
    }
    params
        .as_str(key)
        .map(Some)
        .ok_or_else(|| format!("`{key}` must be text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(scale: Vec3) -> ElementTransform {
        ElementTransform {
            translate: Vec3::ZERO,
            rotate: Quat::IDENTITY,
            scale,
            pivot: Pivot::Centroid,
            space: Space::World,
            falloff: None,
        }
    }

    #[test]
    fn only_vertices_that_move_are_returned() {
        let positions = [
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::ZERO,
        ];
        let (indices, moved) = transform(Vec3::new(0.5, 1.0, 1.0)).moves(
            &positions,
            &[true, true, false],
            Affine3A::IDENTITY,
            Vec3::Y,
        );
        assert_eq!(indices, vec![0, 1]);
        assert_eq!(
            moved,
            vec![Vec3::new(-0.5, 1.0, 0.0), Vec3::new(0.5, 1.0, 0.0)]
        );
    }

    #[test]
    fn world_space_converts_through_the_brush_placement() {
        let positions = [Vec3::X];
        let mut shift = transform(Vec3::ONE);
        shift.translate = Vec3::X;
        let doubled = Affine3A::from_scale(Vec3::splat(2.0));
        let (_, moved) = shift.moves(&positions, &[true], doubled, Vec3::X * 2.0);
        assert!(
            moved[0].distance(Vec3::new(1.5, 0.0, 0.0)) < 1e-6,
            "{moved:?}"
        );
        shift.space = Space::Local;
        let (_, moved) = shift.moves(&positions, &[true], doubled, Vec3::X * 2.0);
        assert!(
            moved[0].distance(Vec3::new(2.0, 0.0, 0.0)) < 1e-6,
            "{moved:?}"
        );
    }

    #[test]
    fn an_identity_far_from_the_origin_moves_nothing() {
        let far = Affine3A::from_translation(Vec3::new(5000.0, 0.0, 0.0));
        let positions = [Vec3::new(0.5, 0.5, 0.5), Vec3::new(-0.5, 0.5, 0.5)];
        let pivot = far.transform_point3(Vec3::new(0.0, 0.5, 0.5));
        let (indices, _) = transform(Vec3::ONE).moves(&positions, &[true, true], far, pivot);
        assert!(indices.is_empty(), "{indices:?}");
    }

    #[test]
    fn a_falloff_rotation_past_half_a_turn_keeps_its_direction() {
        let mut turn = transform(Vec3::ONE);
        turn.rotate = Quat::from_rotation_y(270_f32.to_radians());
        turn.pivot = Pivot::Origin;
        turn.falloff = Some(Falloff {
            radius: 2.0,
            curve: FalloffCurve::Linear,
        });
        let positions = [Vec3::new(0.0, 0.0, 1.0), Vec3::X];
        let (_, moved) = turn.moves(&positions, &[true, false], Affine3A::IDENTITY, Vec3::ZERO);
        let half = 1.0 - 2_f32.sqrt() / 2.0;
        let expected = Quat::from_rotation_y((270.0 * half).to_radians()) * Vec3::X;
        assert!(moved[1].distance(expected) < 1e-5, "{moved:?}");
    }

    #[test]
    fn falloff_curves_run_from_one_at_the_selection_to_zero_at_the_radius() {
        for curve in [
            FalloffCurve::Smooth,
            FalloffCurve::Linear,
            FalloffCurve::Sharp,
            FalloffCurve::Root,
        ] {
            assert_eq!(curve.weight(0.0), 0.0, "{curve:?}");
            assert_eq!(curve.weight(1.0), 1.0, "{curve:?}");
        }
        assert!((FalloffCurve::Smooth.weight(0.5) - 0.5).abs() < 1e-6);
        assert!((FalloffCurve::Sharp.weight(0.5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_flattened_ring_has_no_area() {
        let square = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z];
        assert!((ring_area(&square) - 1.0).abs() < 1e-6);
        let line = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0, Vec3::X];
        assert!(ring_area(&line) <= MIN_FACE_AREA);
    }
}
