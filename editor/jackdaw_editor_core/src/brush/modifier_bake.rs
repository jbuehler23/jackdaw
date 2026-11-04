//! Folding a brush's modifier stack into its authored topology, shared by the
//! modifier and mirror operators and by the CSG and clip operators, which bake
//! a brush's stack before cutting it.

use bevy::prelude::*;
use jackdaw_geometry::{
    EvaluatedBrush, Modifier, ModifierStack, build_topology_from_face_polygons,
    evaluate_modifier_stack,
};
use jackdaw_scene_types::{Brush, BrushFaceData};

use super::BrushHalfedge;

/// Authored vertex positions + per-face vertex rings from `brush.topology`,
/// the input shape `evaluate_mirror` and `evaluate_modifier_stack` expect.
pub fn authored_geometry(brush: &Brush) -> (Vec<Vec3>, Vec<Vec<usize>>) {
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
pub fn rebuild_brush_from_eval(brush: &mut Brush, eval: &EvaluatedBrush) {
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

/// Fold `modifiers` over the brush's authored topology and write the result
/// back as the new authored topology. A no-op when the fold is identity.
pub fn bake_modifier_stack(brush: &mut Brush, modifiers: &[&Modifier]) {
    if brush.topology.polygons.is_empty() {
        return;
    }
    let (vertices, face_polygons) = authored_geometry(brush);
    let eval = evaluate_modifier_stack(&vertices, &face_polygons, &brush.faces, modifiers);
    rebuild_brush_from_eval(brush, &eval);
}

/// Bake `entity`'s whole modifier stack into its authored topology and remove
/// the component. After baking, the authored topology holds the evaluated
/// result, so the stack must go or the next mesh rebuild would fold it again.
/// A stale `BrushHalfedge` would overwrite the new topology on its next
/// flatten, so it is re-lifted when present (same handling as `apply_brush`).
pub fn bake_modifier_stack_entity(world: &mut World, entity: Entity) {
    let Some(stack) = world.get::<ModifierStack>(entity).cloned() else {
        return;
    };
    let relift = world.get::<BrushHalfedge>(entity).is_some();
    let Some(mut brush) = world.get_mut::<Brush>(entity) else {
        return;
    };
    let mods: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
    bake_modifier_stack(&mut brush, &mods);
    let baked = brush.clone();
    if relift {
        world
            .entity_mut(entity)
            .insert(BrushHalfedge::from_topology(&baked.topology));
    }
    world.entity_mut(entity).remove::<ModifierStack>();
    {
        let mut document = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = document.ast_for(entity) {
            document.remove_component_patch(node, ModifierStack::type_path());
        }
    }
    crate::brush::sync_brush_to_ast(world, entity, &baked);
}
