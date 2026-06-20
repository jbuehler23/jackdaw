use crate::commands::CommandHistory;
use crate::draw_brush::{DrawBrushState, env_allows_brush_op};
use crate::keybind_focus::KeybindFocus;
use crate::prelude::*;
use crate::{
    brush::{BrushMeshCache, BrushMeshChunk},
    selection::Selection,
    viewport::ViewportCursor,
};
use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
};
use jackdaw_geometry::{
    brush_planes_to_world, clean_degenerate_faces, compute_brush_geometry_from_planes,
    compute_brush_topology,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

/// `brush.extend_face_to_brush` needs either (a) Face edit mode with a
/// face picked and another brush selected, or (b) Object mode with >= 2
/// brushes selected and a remembered/hovered face on the primary.
fn can_run_extend_face(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    brush_selection: Res<crate::brush::BrushSelection>,
    edit_mode: Res<crate::brush::EditMode>,
    brushes: Query<(), With<Brush>>,
) -> bool {
    if !env_allows_brush_op(&keybind_focus, &modal, &draw_state) {
        return false;
    }
    let brush_count = selection
        .entities
        .iter()
        .filter(|&&e| brushes.contains(e))
        .count();
    match *edit_mode {
        crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face) => {
            brush_selection.active_brush.is_some()
                && brush_selection
                    .active_sub()
                    .is_some_and(|s| !s.faces.is_empty())
                && brush_count >= 1
        }
        crate::brush::EditMode::Object => {
            brush_count >= 2
                && brush_selection.last_face_entity.is_some_and(|e| {
                    brush_selection.last_face_index.is_some() && brushes.contains(e)
                })
        }
        _ => false,
    }
}

#[operator(
    id = "brush.extend_face_to_brush",
    label = "Extend to Brush",
    description = "Extend a face of the primary brush to conform to the shape of \
                   the other selected brushes. Two entry paths:\n\
                   - `EditMode::BrushEdit(Face)` with a face selected on \
                     `BrushSelection` and >= 1 other brush in `Selection::entities`.\n\
                   - `EditMode::Object` with >= 2 brushes in `Selection::entities` \
                     and a remembered face on the primary.\n\
                   Availability (`can_run_extend_face`) is false when neither \
                   entry path applies. The Object-mode path additionally \
                   tries to resolve a hovered face via raycast once invoked; \
                   if that also fails it returns `Cancelled`.",
    is_available = can_run_extend_face,
)]
pub(crate) fn brush_extend_face_to_brush(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    selection: Res<Selection>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    vp: ViewportCursor,
    mut ray_cast: MeshRayCast,
    brush_chunks: Query<&BrushMeshChunk>,
    brush_caches: Query<&BrushMeshCache>,
    brush_query: Query<(), With<Brush>>,
    mut commands: Commands,
) -> OperatorResult {
    // Resolve (primary, face_index, targets) depending on edit mode
    let (primary, face_index, targets) = if *edit_mode
        == crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face)
    {
        // Face mode path: primary is the brush being edited, face is the selected face
        let primary = brush_selection
            .active_brush
            .filter(|&e| brush_query.contains(e))?;
        let face_index = brush_selection
            .sub(primary)
            .and_then(|s| s.faces.last().copied())?;
        let targets: Vec<Entity> = selection
            .entities
            .iter()
            .copied()
            .filter(|&e| e != primary && brush_query.contains(e))
            .collect();
        if targets.is_empty() {
            return OperatorResult::Cancelled;
        }
        (primary, face_index, targets)
    } else if *edit_mode == crate::brush::EditMode::Object {
        // Object mode: need 2+ brushes selected
        let selected_brushes: Vec<Entity> = selection
            .entities
            .iter()
            .copied()
            .filter(|&e| brush_query.contains(e))
            .collect();
        if selected_brushes.len() < 2 {
            return OperatorResult::Cancelled;
        }

        let primary = selection.primary().filter(|e| brush_query.contains(*e))?;
        let targets: Vec<Entity> = selected_brushes
            .into_iter()
            .filter(|&e| e != primary)
            .collect();

        // Try hover raycast first to find the face
        let face_index =
            find_hovered_face_on_brush(primary, &vp, &mut ray_cast, &brush_chunks, &brush_caches)
                .or_else(|| {
                    // Fall back to remembered face
                    if brush_selection.last_face_entity == Some(primary) {
                        brush_selection.last_face_index
                    } else {
                        None
                    }
                });

        let face_index = face_index?;
        (primary, face_index, targets)
    } else {
        return OperatorResult::Cancelled;
    };

    // If we were in face mode, exit it (geometry is about to change, indices become invalid)
    if *edit_mode == crate::brush::EditMode::BrushEdit(crate::brush::BrushEditMode::Face) {
        *edit_mode = crate::brush::EditMode::Object;
        brush_selection.clear();
    }

    let targets_clone = targets.clone();
    commands.queue(move |world: &mut World| {
        extend_face_to_brush_impl(world, primary, &targets_clone, face_index);
    });
    OperatorResult::Finished
}

/// Raycast from cursor to find a hovered brush face belonging to the given
/// brush. Returns the authored face index if found; a hit on a mirrored
/// copy resolves to its source face.
fn find_hovered_face_on_brush(
    brush_entity: Entity,
    vp: &ViewportCursor,
    ray_cast: &mut MeshRayCast,
    brush_chunks: &Query<&BrushMeshChunk>,
    brush_caches: &Query<&BrushMeshCache>,
) -> Option<usize> {
    let cursor_pos = vp.cursor()?;
    let camera_entity = vp.camera_entity()?;
    let viewport_entity = vp.viewport_entity()?;
    let (camera, cam_tf) = vp.camera_for(camera_entity)?;
    let viewport_cursor = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos)?;
    let ray = camera.viewport_to_world(cam_tf, viewport_cursor).ok()?;

    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
    let hits = ray_cast.cast_ray(ray, &settings);

    for (hit_entity, hit_data) in hits {
        let Ok(chunk) = brush_chunks.get(*hit_entity) else {
            continue;
        };
        if chunk.brush_entity != brush_entity {
            continue;
        }
        let Some(tri_idx) = hit_data.triangle_index else {
            continue;
        };
        let Some(&face_idx) = chunk.face_of_tri.get(tri_idx) else {
            continue;
        };
        // A hit on a bisect cut cap has no authored face and is skipped.
        let face_idx = match brush_caches.get(brush_entity) {
            Ok(cache) => {
                let Some(authored) = cache.authored_face(face_idx as usize) else {
                    continue;
                };
                authored
            }
            Err(_) => face_idx as usize,
        };
        return Some(face_idx);
    }
    None
}

/// Core logic for Extend Face to Brush.
///
/// Removes the specified face from the primary brush, adds all target brush faces,
/// then computes the intersection. The result is the primary brush reshaped to
/// conform to the target brushes in the direction of the removed face.
pub(crate) fn extend_face_to_brush_impl(
    world: &mut World,
    primary: Entity,
    targets: &[Entity],
    face_index: usize,
) {
    // Read primary brush
    let Some(primary_brush) = world.get::<Brush>(primary) else {
        return;
    };
    let old_brush = primary_brush.clone();
    if face_index >= old_brush.faces.len() {
        return;
    }

    let Some(primary_gtf) = world.get::<GlobalTransform>(primary) else {
        return;
    };
    let (_, rotation, translation) = primary_gtf.to_scale_rotation_translation();
    let inv_rotation = rotation.inverse();

    // Transform primary faces to world space, removing the target face
    let all_world_faces = brush_planes_to_world(&old_brush.faces, rotation, translation);
    let removed_normal = all_world_faces[face_index].plane.normal;
    let mut world_faces: Vec<BrushFaceData> = all_world_faces
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != face_index)
        .map(|(_, f)| f)
        .collect();

    // Collect candidate target faces in world space
    let mut candidate_faces = Vec::new();
    for &target in targets {
        let Some(target_brush) = world.get::<Brush>(target) else {
            continue;
        };
        let Some(target_gtf) = world.get::<GlobalTransform>(target) else {
            continue;
        };
        let (_, t_rot, t_trans) = target_gtf.to_scale_rotation_translation();
        let target_world_faces = brush_planes_to_world(&target_brush.faces, t_rot, t_trans);
        // Flip target faces: negate normal and distance so the half-space constraint
        // means "on the outside of the target brush" rather than "inside it". This way
        // the wall extends UP TO the target surface instead of being clipped to the
        // target interior.
        candidate_faces.extend(target_world_faces.into_iter().map(|f| BrushFaceData {
            plane: BrushPlane {
                normal: -f.plane.normal,
                distance: -f.plane.distance,
            },
            ..f
        }));
    }

    // Filter target faces: prefer angled faces (not anti-parallel or perpendicular to the
    // removed face). Anti-parallel faces (dot approx -1) would just re-cap at the same level,
    // and perpendicular/same-direction faces (dot >= 0) don't constrain the extension.
    let angled: Vec<BrushFaceData> = candidate_faces
        .iter()
        .filter(|f| {
            let dot = f.plane.normal.dot(removed_normal);
            dot < -0.01 && dot > -0.99
        })
        .cloned()
        .collect();

    // If we found angled faces, use those. Otherwise fall back to all faces with a
    // negative dot (the simple flat-ceiling case where anti-parallel IS the constraint).
    if !angled.is_empty() {
        world_faces.extend(angled);
    } else {
        let opposing: Vec<BrushFaceData> = candidate_faces
            .into_iter()
            .filter(|f| f.plane.normal.dot(removed_normal) < -0.01)
            .collect();
        world_faces.extend(opposing);
    }

    // Compute geometry from combined face set
    let (verts, _) = compute_brush_geometry_from_planes(&world_faces);
    if verts.len() < 4 {
        return;
    }

    // No-op check: compare with original geometry
    let (old_verts, _) = compute_brush_geometry_from_planes(&brush_planes_to_world(
        &old_brush.faces,
        rotation,
        translation,
    ));
    if verts.len() == old_verts.len() {
        let mut changed = false;
        for (a, b) in verts.iter().zip(old_verts.iter()) {
            if a.distance(*b) > 1e-4 {
                changed = true;
                break;
            }
        }
        if !changed {
            return;
        }
    }

    // Convert ALL world faces back to local space (keeping constraint planes),
    // then clean degenerate faces once in local space.
    let local_faces: Vec<BrushFaceData> = world_faces
        .iter()
        .map(|f| BrushFaceData {
            plane: BrushPlane {
                normal: inv_rotation * f.plane.normal,
                distance: f.plane.distance - f.plane.normal.dot(translation),
            },
            ..f.clone()
        })
        .collect();
    let local_clean = clean_degenerate_faces(&local_faces);
    if local_clean.len() < 4 {
        return;
    }
    // Apply via undo-able SetBrush command (ECS + AST)
    let topology = compute_brush_topology(&local_clean);
    let new_brush = Brush {
        faces: local_clean,
        topology,
    };
    crate::brush::sync_brush_to_ast(world, primary, &new_brush);
    if let Some(mut brush) = world.get_mut::<Brush>(primary) {
        *brush = new_brush.clone();
    }

    let cmd = crate::brush::SetBrush {
        entity: primary,
        old: old_brush,
        new: new_brush,
        label: "Extend face to brush".to_string(),
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}
