//! `mesh.mirror.*` and `mesh.symmetrize` operators: manage the brush's
//! `ModifierStack` Mirror entries and bake their evaluated geometry into the
//! authored topology.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::bisect_plane::BisectKeep;
use jackdaw_geometry::{
    EvaluatedBrush, MeshMirror, Modifier, ModifierEntry, ModifierStack,
    build_topology_from_face_polygons, evaluate_mirror,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

use crate::brush::BrushHalfedge;
use crate::clip_ops::bisect_brush;
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::{DrawBrushState, env_allows_brush_op};
use crate::keybind_focus::KeybindFocus;
use crate::modifier_ops::bake_modifier_stack;
use crate::selection::Selection;

/// Authored vertex positions + per-face vertex rings from `brush.topology`,
/// the input shape `evaluate_mirror` and `evaluate_modifier_stack` expect.
pub(crate) fn authored_geometry(brush: &Brush) -> (Vec<Vec3>, Vec<Vec<usize>>) {
    let vertices: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
    let face_polygons: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
        .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
        .collect();
    (vertices, face_polygons)
}

/// Rebuild `brush.topology` + `brush.faces` + face planes + edge flags from
/// an evaluated result: duplicate authored face data into the appended slots
/// via `face_source`, carry edge flags through `vert_source`, and recompute
/// each face plane from the new topology. Shared by the single-mirror bake
/// and the full `ModifierStack` bake. Does nothing when the evaluation was
/// identity (empty source maps).
pub(crate) fn rebuild_brush_from_eval(brush: &mut Brush, eval: &EvaluatedBrush) {
    if eval.vert_source.is_empty() {
        return;
    }

    debug_assert_eq!(
        brush.faces.len(),
        brush.topology.polygons.len(),
        "brush.faces and topology.polygons must be parallel arrays"
    );
    let new_faces: Vec<BrushFaceData> = eval
        .face_source
        .iter()
        .map(|&src| brush.faces.get(src as usize).cloned().unwrap_or_default())
        .collect();
    let mut new_topology =
        build_topology_from_face_polygons(eval.vertices.clone(), eval.face_polygons.clone());

    // Carry edge flags (sharp / seam) over: map each rebuilt edge's verts
    // back to authored verts through `vert_source` and look the pair up in
    // the old topology, so mirrored edges inherit their source's flags.
    for edge in &mut new_topology.edges {
        let a = eval.vert_source[edge.v[0] as usize];
        let b = eval.vert_source[edge.v[1] as usize];
        if let Some(old_idx) = brush.topology.edge_id(a, b) {
            edge.flags = brush.topology.edges[old_idx as usize].flags;
        }
    }

    // Update plane data per face from the new topology; mirrored faces
    // have reflected normals relative to their source.
    let mut faces = new_faces;
    new_topology.recompute_face_planes(&mut faces);
    brush.faces = faces;
    brush.topology = new_topology;
}

/// Bake `mirror` into the brush's authored topology: evaluate the mirror and
/// rebuild from the result. Shared by `mesh.symmetrize` and the CSG impls.
pub(crate) fn apply_mirror_to_brush(brush: &mut Brush, mirror: &MeshMirror) {
    if mirror.axes().is_empty() || brush.topology.polygons.is_empty() {
        return;
    }
    let (vertices, face_polygons) = authored_geometry(brush);
    let eval = evaluate_mirror(&vertices, &face_polygons, mirror);
    rebuild_brush_from_eval(brush, &eval);
}

/// Whether `stack` carries at least one `Modifier::Mirror` entry.
fn stack_has_mirror(stack: Option<&ModifierStack>) -> bool {
    stack.is_some_and(|s| {
        s.modifiers
            .iter()
            .any(|e| matches!(e.modifier, Modifier::Mirror(_)))
    })
}

/// Bake `entity`'s modifier stack into its authored topology and remove the
/// component. After baking, the authored topology holds the evaluated result,
/// so the stack must go or the next mesh rebuild would fold it again.
pub(crate) fn bake_mirror(world: &mut World, entity: Entity) {
    crate::modifier_ops::bake_modifier_stack_entity(world, entity);
}

/// Bake modifier stacks ahead of a CSG pass that reads the whole scene:
/// `cutters` bake unconditionally; other stacked brushes bake only when their
/// evaluated bounds reach a cutter, so far-away brushes keep their live stack
/// instead of getting silently baked by an unrelated cut.
pub(crate) fn bake_engaged_mirrors(world: &mut World, cutters: &[Entity]) {
    for &entity in cutters {
        bake_mirror(world, entity);
    }

    let mut brush_query = world.query::<(Entity, &Brush, &GlobalTransform)>();
    let cutter_aabbs: Vec<(Vec3, Vec3)> = cutters
        .iter()
        .filter_map(|&e| {
            let (_, brush, gt) = brush_query.get(world, e).ok()?;
            world_aabb(brush.topology.vertices.iter().map(|v| v.position), gt)
        })
        .collect();
    if cutter_aabbs.is_empty() {
        return;
    }

    let mut stacked_query = world.query::<(Entity, &Brush, &ModifierStack, &GlobalTransform)>();
    let engaged: Vec<Entity> = stacked_query
        .iter(world)
        .filter(|(entity, ..)| !cutters.contains(entity))
        .filter_map(|(entity, brush, stack, gt)| {
            let (vertices, face_polygons) = authored_geometry(brush);
            let mods: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
            let eval = jackdaw_geometry::evaluate_modifier_stack(
                &vertices,
                &face_polygons,
                &brush.faces,
                &mods,
            );
            let aabb = world_aabb(eval.vertices.into_iter(), gt)?;
            cutter_aabbs
                .iter()
                .any(|cutter| aabbs_overlap(cutter, &aabb))
                .then_some(entity)
        })
        .collect();
    for entity in engaged {
        bake_mirror(world, entity);
    }
}

fn world_aabb(points: impl Iterator<Item = Vec3>, gt: &GlobalTransform) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::MAX;
    let mut max = Vec3::MIN;
    let mut any = false;
    for p in points {
        let w = gt.transform_point(p);
        min = min.min(w);
        max = max.max(w);
        any = true;
    }
    any.then_some((min, max))
}

/// Same epsilon as `topology_aabbs_overlap` in `draw_brush`.
fn aabbs_overlap(a: &(Vec3, Vec3), b: &(Vec3, Vec3)) -> bool {
    const E: f32 = 1e-4;
    a.0.x <= b.1.x + E
        && a.1.x >= b.0.x - E
        && a.0.y <= b.1.y + E
        && a.1.y >= b.0.y - E
        && a.0.z <= b.1.z + E
        && a.1.z >= b.0.z - E
}

/// Add a default mirror entry (X axis, clipped) to every selected brush that
/// doesn't already carry one. Available with a mirror-less brush selected.
#[operator(
    id = "mesh.mirror.add",
    label = "Add Mirror",
    is_available = can_add_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_add(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    let mut added = false;
    for &entity in &selection.entities {
        let Ok((_, stack)) = candidates.get(entity) else {
            continue;
        };
        if stack_has_mirror(stack) {
            continue;
        }
        let mut stack = stack.cloned().unwrap_or_default();
        stack
            .modifiers
            .push(ModifierEntry::new(Modifier::Mirror(MeshMirror::default())));
        commands.entity(entity).insert(stack);
        added = true;
    }
    if added {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Bake the modifier stack of every selected mirrored brush into its authored
/// topology and remove the component. Available with a mirrored brush
/// selected.
#[operator(
    id = "mesh.mirror.apply",
    label = "Apply Mirror",
    is_available = can_apply_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_apply(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, &ModifierStack)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let mut applied = false;
    for &entity in &selection.entities {
        let Ok((mut brush, stack)) = brushes.get_mut(entity) else {
            continue;
        };
        let mods: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
        bake_modifier_stack(&mut brush, &mods);
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        commands.entity(entity).remove::<ModifierStack>();
        applied = true;
    }
    if applied {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Cut every selected mirror-less brush at the default mirror plane (brush
/// local X through zero), keep the positive side, and add a default mirror
/// entry so the discarded half re-appears as the mirrored copy. Available
/// with a mirror-less brush selected.
#[operator(
    id = "mesh.mirror.bisect",
    label = "Bisect and Mirror",
    is_available = can_add_mirror,
    allows_undo = true
)]
pub(crate) fn mesh_mirror_bisect(
    _: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, Option<&ModifierStack>)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let plane = BrushPlane {
        normal: Vec3::X,
        distance: 0.0,
    };
    let mut cut = false;
    for &entity in &selection.entities {
        let Ok((mut brush, stack)) = brushes.get_mut(entity) else {
            continue;
        };
        if stack_has_mirror(stack) {
            continue;
        }
        let Some(half) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
            continue;
        };
        *brush = half;
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        let mut stack = stack.cloned().unwrap_or_default();
        stack
            .modifiers
            .push(ModifierEntry::new(Modifier::Mirror(MeshMirror::default())));
        commands.entity(entity).insert(stack);
        cut = true;
    }
    if cut {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// Make every selected brush symmetric across a brush-local axis plane in
/// one step: any live mirror is first baked into the authored topology, then
/// the result is cut at the plane keeping the positive side, mirror-evaluated,
/// and written back as authored topology. No live mirror is left behind; any
/// existing one is removed since the authored topology now holds both halves.
/// Available with a brush selected.
#[operator(
    id = "mesh.symmetrize",
    label = "Symmetrize",
    is_available = can_symmetrize,
    allows_undo = true,
    params(
        axis(String, default = "x", doc = "Brush-local mirror axis: \"x\", \"y\", or \"z\"."),
    ),
)]
pub(crate) fn mesh_symmetrize(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, Option<&ModifierStack>)>,
    halfedges: Query<(), With<BrushHalfedge>>,
    mut commands: Commands,
) -> OperatorResult {
    let axis_str = params.as_str("axis").unwrap_or("x");
    let axis = match axis_str {
        "y" => 1,
        "z" => 2,
        "x" => 0,
        other => {
            warn!("mesh.symmetrize: unrecognized axis \"{other}\", falling back to \"x\"");
            0
        }
    };
    let mut normal = Vec3::ZERO;
    normal[axis] = 1.0;
    let plane = BrushPlane {
        normal,
        distance: 0.0,
    };
    let mirror = MeshMirror {
        mirror_x: axis == 0,
        mirror_y: axis == 1,
        mirror_z: axis == 2,
        ..MeshMirror::default()
    };

    let mut symmetrized = false;
    for &entity in &selection.entities {
        let Ok((mut brush, existing_stack)) = brushes.get_mut(entity) else {
            continue;
        };
        let had_stack = existing_stack.is_some();
        if let Some(existing) = existing_stack {
            let mods: Vec<&Modifier> = existing.modifiers.iter().map(|e| &e.modifier).collect();
            bake_modifier_stack(&mut brush, &mods);
        }
        let Some(half) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
            continue;
        };
        *brush = half;
        apply_mirror_to_brush(&mut brush, &mirror);
        if halfedges.contains(entity) {
            commands
                .entity(entity)
                .insert(BrushHalfedge::from_topology(&brush.topology));
        }
        if had_stack {
            commands.entity(entity).remove::<ModifierStack>();
        }
        symmetrized = true;
    }
    if symmetrized {
        OperatorResult::Finished
    } else {
        OperatorResult::Cancelled
    }
}

/// `mesh.mirror.add` / `mesh.mirror.bisect` need a selected brush without
/// a Mirror entry in its stack.
pub(crate) fn can_add_mirror(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|(_, stack)| !stack_has_mirror(stack))
        })
}

/// `mesh.mirror.apply` needs a selected brush carrying a Mirror entry.
pub(crate) fn can_apply_mirror(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(Entity, Option<&ModifierStack>), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| {
            candidates
                .get(e)
                .is_ok_and(|(_, stack)| stack_has_mirror(stack))
        })
}

/// `mesh.symmetrize` needs any selected brush.
pub(crate) fn can_symmetrize(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    candidates: Query<(), With<Brush>>,
) -> bool {
    env_allows_brush_op(&keybind_focus, &modal, &draw_state)
        && selection.entities.iter().any(|&e| candidates.contains(e))
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<MeshMirrorAddOp>()
        .register_operator::<MeshMirrorApplyOp>()
        .register_operator::<MeshMirrorBisectOp>()
        .register_operator::<MeshSymmetrizeOp>();
    // No default keybinds; the unbound action entities keep the ops
    // bindable by presets and user rebinds.
    ctx.action_for::<CoreExtensionInputContext, MeshMirrorAddOp>();
    ctx.action_for::<CoreExtensionInputContext, MeshMirrorApplyOp>();
    ctx.action_for::<CoreExtensionInputContext, MeshMirrorBisectOp>();
    ctx.action_for::<CoreExtensionInputContext, MeshSymmetrizeOp>();
}
