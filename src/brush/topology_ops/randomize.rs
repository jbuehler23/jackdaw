//! `brush.mesh.randomize`: move the vertices of one or more brushes by seeded
//! random offsets, each shared vertex once, so rocks, cobbles and planks get
//! an irregular shape from one call.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{Jitter, MirrorAxes, jitter_offsets, newell_normal};
use jackdaw_scene_types::{Brush, SceneNodeId};

use super::element_transform::{
    MOVE_TOLERANCE, Space, Target, is_element_mode, targets as element_targets, unchanged,
};
use super::select_elements::{edit_target, refuse};
use crate::brush::{BrushSelection, EditMode};
use crate::numeric_transform::sub_selection_vertices;
use crate::primitive_ops::{bool_param, float_param, int_param, vec3_param};

const LABEL: &str = "Randomize Vertices";

/// Largest `amount` a call may ask for.
const MAX_AMOUNT: f64 = 1000.0;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushMeshRandomizeOp>();
}

#[operator(
    id = "brush.mesh.randomize",
    label = "Randomize Vertices",
    description = "Move vertices by seeded random offsets no longer than `amount`: the \
                   selected faces, edges or vertices, else every vertex of the brush. A vertex \
                   shared by several faces moves once, so the brush stays closed.",
    allows_undo = true,
    params(
        entity(
            Entity,
            doc = "Brush to randomize. Defaults to the brushes being edited with a selection, \
                   else every selected brush."
        ),
        amount(
            f64,
            doc = "Longest distance a vertex moves, in `space` units, 0 or more."
        ),
        seed(
            i64,
            doc = "Random seed, 0 to 4294967295; the same seed gives a brush the same shape. \
                   Defaults to 0."
        ),
        axis_mask(
            Vec3,
            doc = "Factor per axis [x, y, z], each 0 to 1, in `space`. Defaults to [1, 1, 1]."
        ),
        normal_only(
            bool,
            doc = "Move along each vertex normal, in or out, rather than in any direction. \
                   Defaults to false."
        ),
        space(
            String,
            doc = "world or local (each brush's own axes). Defaults to world."
        ),
    )
)]
pub(crate) fn brush_mesh_randomize(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let request = match Randomize::read(&params) {
        Ok(request) => request,
        Err(why) => return refuse(world, LABEL, why),
    };
    let targets = match targets(world, &params) {
        Ok(targets) => targets,
        Err(Stop::Invalid(why)) => return refuse(world, LABEL, why),
        Err(Stop::Nothing(why)) => return unchanged(world, LABEL, &why),
    };
    let mut edits = Vec::new();
    for target in &targets {
        let stream = world.get::<SceneNodeId>(target.entity).map_or(0, |id| id.0);
        let before = target.positions();
        let (indices, positions) = request.moves(target, &before, stream);
        match target.reshape(&before, &indices, &positions) {
            Ok(Some(edit)) => {
                if let Some(why) = flipped_face(target, &before, &edit.brush, &indices) {
                    return refuse(world, LABEL, why);
                }
                edits.push(edit);
            }
            Ok(None) => {}
            Err(why) => return refuse(world, LABEL, why),
        }
    }
    if edits.is_empty() {
        return unchanged(world, LABEL, "no vertex moved");
    }
    super::element_transform::commit(world, LABEL, edits)
}

/// Why a call ends before any brush changes.
enum Stop {
    Invalid(String),
    Nothing(String),
}

/// The brushes a call acts on: `entity`, else the brushes being edited with
/// a selection, else the selected brushes. A brush being edited in face,
/// edge or vertex mode gives its selected vertices; otherwise every vertex.
fn targets(world: &World, params: &OperatorParameters) -> Result<Vec<Target>, Stop> {
    let element_mode = is_element_mode(*world.resource::<EditMode>());
    let nothing = || Stop::Nothing("nothing is selected to randomize".into());
    if params.contains_key("entity") {
        let entity = edit_target(world, params.get("entity")).map_err(Stop::Invalid)?;
        let vertex_count = world
            .get::<Brush>(entity)
            .map_or(0, |brush| brush.topology.vertices.len());
        if vertex_count == 0 {
            return Err(Stop::Invalid(format!("brush {entity} has no vertices")));
        }
        let sub = world
            .resource::<BrushSelection>()
            .sub(entity)
            .filter(|_| element_mode);
        let Some(sub) = sub else {
            let target = Target::new(world, entity, |_, _| (0..vertex_count).collect());
            return target.map(|target| vec![target]).ok_or_else(nothing);
        };
        if sub.faces.is_empty() && sub.edges.is_empty() && sub.vertices.is_empty() {
            return Err(nothing());
        }
        return Target::new(world, entity, |_, rings| sub_selection_vertices(sub, rings))
            .map(|target| vec![target])
            .ok_or_else(|| {
                Stop::Invalid(format!(
                    "the selection on brush {entity} names no vertex it has"
                ))
            });
    }
    if element_mode {
        let edited = element_targets(world);
        if edited.is_empty() {
            return Err(nothing());
        }
        return Ok(edited);
    }
    let mut selected: Vec<Entity> = world
        .get_resource::<crate::selection::Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default();
    selected.sort();
    selected.dedup();
    let targets: Vec<Target> = selected
        .into_iter()
        .filter_map(|entity| {
            Target::new(world, entity, |brush, _| {
                (0..brush.topology.vertices.len()).collect()
            })
        })
        .collect();
    if targets.is_empty() {
        return Err(Stop::Invalid(
            "name a brush with `entity`; none is being edited or selected".into(),
        ));
    }
    Ok(targets)
}

/// Why `after` cannot stand: the first face touching a vertex in `moved`
/// whose normal turned to face the other way.
fn flipped_face(
    target: &Target,
    before: &[Vec3],
    after: &Brush,
    moved: &[usize],
) -> Option<String> {
    let after_positions: Vec<Vec3> = after
        .topology
        .vertices
        .iter()
        .map(|vert| vert.position)
        .collect();
    let mut is_moved = vec![false; before.len()];
    for &vert in moved {
        if let Some(slot) = is_moved.get_mut(vert) {
            *slot = true;
        }
    }
    target.rings.iter().enumerate().find_map(|(face, ring)| {
        if !ring
            .iter()
            .any(|&vert| is_moved.get(vert).copied().unwrap_or(false))
        {
            return None;
        }
        let normal = |positions: &[Vec3]| {
            let points: Vec<Vec3> = ring
                .iter()
                .filter_map(|&vert| positions.get(vert).copied())
                .collect();
            newell_normal(&points)
        };
        (normal(before).dot(normal(&after_positions)) <= 0.0).then(|| {
            format!(
                "face {face} of brush {} would turn inside out; use a smaller `amount`",
                target.entity
            )
        })
    })
}

/// A `brush.mesh.randomize` call, checked.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Randomize {
    jitter: Jitter,
    space: Space,
}

impl Randomize {
    fn read(params: &OperatorParameters) -> Result<Self, String> {
        let amount = float_param(params, "amount", 0.0..=MAX_AMOUNT)?
            .ok_or("give `amount`, the longest distance a vertex moves")?;
        let seed = int_param(params, "seed", 0..=i64::from(u32::MAX))?.unwrap_or(0) as u32;
        let axis_mask = vec3_param(params, "axis_mask")?.unwrap_or(Vec3::ONE);
        if axis_mask.min_element() < 0.0 || axis_mask.max_element() > 1.0 {
            return Err(format!(
                "`axis_mask` {axis_mask} must be between 0 and 1 on every axis"
            ));
        }
        if axis_mask == Vec3::ZERO {
            return Err("`axis_mask` is zero on every axis, so nothing could move".into());
        }
        Ok(Self {
            jitter: Jitter {
                amount,
                seed,
                axis_mask,
                normal_only: bool_param(params, "normal_only")?.unwrap_or(false),
            },
            space: Space::read(params)?,
        })
    }

    /// The vertices of `target` that move and where to, brush-local, from
    /// its brush-local `positions`. A vertex on an enabled mirror plane stays
    /// on it.
    fn moves(&self, target: &Target, positions: &[Vec3], stream: u64) -> (Vec<usize>, Vec<Vec3>) {
        let to_space = self.space.local_to_space(target.to_world);
        let from_space = to_space.inverse();
        let in_space: Vec<Vec3> = positions
            .iter()
            .map(|&point| to_space.transform_point3(point))
            .collect();
        let offsets = jitter_offsets(
            &in_space,
            &target.rings,
            &target.picked,
            &self.jitter,
            stream,
        );
        let mut indices = Vec::new();
        let mut moved = Vec::new();
        for (vert, offset) in offsets.into_iter().enumerate() {
            let start = positions[vert];
            let size = 1.0 + start.abs().max_element();
            let mut next = start + from_space.transform_vector3(offset);
            if let Some(mirror) = &target.mirror {
                let axes = mirror.axes();
                for (bit, axis) in [(MirrorAxes::X, 0), (MirrorAxes::Y, 1), (MirrorAxes::Z, 2)] {
                    let on_plane = (start[axis] - mirror.offset[axis]).abs()
                        <= mirror.merge_dist.max(MOVE_TOLERANCE * size);
                    if axes.contains(bit) && on_plane {
                        next[axis] = start[axis];
                    }
                }
            }
            if next.distance(start) <= MOVE_TOLERANCE * size {
                continue;
            }
            indices.push(vert);
            moved.push(next);
        }
        (indices, moved)
    }
}
