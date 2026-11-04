//! The non-modal form of the mesh edit operators: amounts given as parameters,
//! applied to the element selection of the brush being edited in one step.
//! The edit runs on copies, so a call that changes nothing touches nothing.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::{EdgeKey, FaceKey, HalfedgeMesh, VertKey, edge_between};
use jackdaw_scene_types::{Brush, BrushTopology};

use super::select_elements::refuse;
use jackdaw_editor_core::brush::{
    BrushEditMode, BrushHalfedge, BrushSelection, BrushSubSelection, EditMode,
};

/// The sub-selection an edit leaves on its brush, replacing the old one.
pub(crate) enum Picks {
    Faces(Vec<usize>),
    Edges(Vec<(usize, usize)>),
    Vertices(Vec<usize>),
}

/// Run `edit` on a copy of the brush being edited in `mode` and a half-edge
/// binding lifted fresh from it, given that brush's selection. `Ok(None)`
/// from `edit` means it changed nothing, and the call reports `unchanged`;
/// `Err` refuses it as `invalid`. Either way the brush is untouched.
/// Otherwise the copies replace the brush, `edit`'s picks replace its
/// selection, and the caller is told how many faces, edges and vertices the
/// edit added. Undo comes from the operator's framework snapshot.
pub(crate) fn run(
    world: &mut World,
    label: &str,
    mode: BrushEditMode,
    edit: impl FnOnce(
        &mut Brush,
        &mut BrushHalfedge,
        &BrushSubSelection,
    ) -> Result<Option<Picks>, String>,
) -> OperatorResult {
    let Some((entity, selected)) = edited(world, mode) else {
        return unchanged(world, format!("{label}: nothing is selected to edit"));
    };
    let Some(mut brush) = world.get::<Brush>(entity).cloned() else {
        return unchanged(world, format!("{label}: {entity} is not a brush"));
    };
    let before = Counts::of(&brush.topology);
    let mut halfedge = BrushHalfedge::from_topology(&brush.topology);
    let picks = match edit(&mut brush, &mut halfedge, &selected) {
        Ok(Some(picks)) => picks,
        Ok(None) => {
            return unchanged(
                world,
                format!("{label}: the selection has nothing this edit applies to"),
            );
        }
        Err(why) => return refuse(world, label, why),
    };
    let report = before.added(&brush.topology);
    if let Some(mut live) = world.get_mut::<Brush>(entity) {
        *live = brush;
    }
    world.entity_mut(entity).insert(halfedge);
    let mut selection = world.resource_mut::<BrushSelection>();
    let sub = selection.sub_mut(entity);
    sub.clear();
    match picks {
        Picks::Faces(faces) => sub.faces = faces,
        Picks::Edges(edges) => sub.edges = edges,
        Picks::Vertices(vertices) => sub.vertices = vertices,
    }
    report_to_caller(world, format!("{label}: {report}"));
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}

/// The brush being edited in `mode` and its selection.
fn edited(world: &World, mode: BrushEditMode) -> Option<(Entity, BrushSubSelection)> {
    if *world.resource::<EditMode>() != EditMode::BrushEdit(mode) {
        return None;
    }
    let selection = world.resource::<BrushSelection>();
    let entity = selection.active_brush?;
    Some((entity, selection.sub(entity)?.clone()))
}

fn unchanged(world: &mut World, why: String) -> OperatorResult {
    warn_caller(world, why);
    set_outcome(world, Outcome::Unchanged);
    OperatorResult::Cancelled
}

/// A brush's face, edge and vertex counts.
#[derive(Clone, Copy)]
pub(crate) struct Counts([usize; 3]);

impl Counts {
    pub(crate) fn of(topology: &BrushTopology) -> Self {
        Self([
            topology.polygons.len(),
            topology.edges.len(),
            topology.vertices.len(),
        ])
    }

    /// How many faces, edges and vertices `after` has beyond these.
    pub(crate) fn added(self, after: &BrushTopology) -> String {
        let [faces, edges, vertices] = Self::of(after).0;
        let [was_faces, was_edges, was_vertices] = self.0;
        format!(
            "added {} faces, {} edges and {} vertices",
            faces.saturating_sub(was_faces),
            edges.saturating_sub(was_edges),
            vertices.saturating_sub(was_vertices),
        )
    }
}

/// A number parameter, `None` when absent.
pub(crate) fn number(params: &OperatorParameters, key: &str) -> Result<Option<f32>, String> {
    if !params.contains_key(key) {
        return Ok(None);
    }
    let value = params
        .as_float(key)
        .ok_or_else(|| format!("`{key}` must be a number"))?;
    if !value.is_finite() {
        return Err(format!("`{key}` must be a finite number"));
    }
    Ok(Some(value as f32))
}

/// A whole-number parameter from 1 to `max`, `None` when absent.
pub(crate) fn count(
    params: &OperatorParameters,
    key: &str,
    max: u32,
) -> Result<Option<u32>, String> {
    if !params.contains_key(key) {
        return Ok(None);
    }
    params
        .as_int(key)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (1..=max).contains(value))
        .map(Some)
        .ok_or_else(|| format!("`{key}` must be a whole number from 1 to {max}"))
}

/// A number parameter that must be present and above zero.
pub(crate) fn positive(params: &OperatorParameters, key: &str) -> Result<f32, String> {
    let value = number(params, key)?.ok_or_else(|| format!("give `{key}`"))?;
    if value <= 0.0 {
        return Err(format!("`{key}` must be above zero"));
    }
    Ok(value)
}

/// `value` of parameter `key`, refused unless it is below `limit`, the most
/// the selection takes.
pub(crate) fn below(value: f32, key: &str, limit: f32) -> Result<f32, String> {
    if value >= limit {
        return Err(format!(
            "`{key}` {value} is too much for the selection; it must be below {limit:.4}"
        ));
    }
    Ok(value)
}

/// The half-edge faces behind the selected face indices.
pub(crate) fn face_keys(halfedge: &BrushHalfedge, faces: &[usize]) -> Vec<FaceKey> {
    faces
        .iter()
        .filter_map(|face| halfedge.face_keys.get(*face).copied())
        .collect()
}

/// The half-edge vertices behind the selected vertex indices.
pub(crate) fn vert_keys(halfedge: &BrushHalfedge, vertices: &[usize]) -> Vec<VertKey> {
    vertices
        .iter()
        .filter_map(|vertex| halfedge.vert_keys.get(*vertex).copied())
        .collect()
}

/// The half-edge edges behind the selected vertex pairs.
pub(crate) fn edge_keys(halfedge: &BrushHalfedge, edges: &[(usize, usize)]) -> Vec<EdgeKey> {
    edges
        .iter()
        .filter_map(|&(a, b)| {
            let va = *halfedge.vert_keys.get(a)?;
            let vb = *halfedge.vert_keys.get(b)?;
            edge_between(&halfedge.mesh, va, vb)
        })
        .collect()
}

/// Run a modal operator's pointer-driven system, for a call that gave none of
/// the operator's parameters.
pub(crate) fn run_modal<M>(
    world: &mut World,
    system: impl IntoSystem<In<OperatorParameters>, Option<OperatorResult>, M> + Send + 'static,
    params: OperatorParameters,
) -> OperatorResult {
    match world.run_system_cached_with(system, params) {
        Ok(result) => result.unwrap_or(OperatorResult::Cancelled),
        Err(err) => {
            error!("a modal mesh edit could not run: {err}");
            OperatorResult::Cancelled
        }
    }
}

/// Where each of `faces` lands once `mesh` is flattened.
pub(crate) fn flattened_indices(mesh: &HalfedgeMesh, faces: &[FaceKey]) -> Vec<usize> {
    let order = mesh.flatten_order();
    faces
        .iter()
        .filter_map(|face| order.iter().position(|key| key == face))
        .collect()
}
