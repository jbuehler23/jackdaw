//! Clip-tool operators. Replace the keybind/click branches in
//! `brush::interaction::handle_clip_mode`. The remaining clip-mode
//! work (recomputing the preview plane and drawing the gizmo
//! overlay) stays in `interaction.rs`.
//!
//! Default keybinds: LMB places a point, Tab cycles mode, Enter
//! applies, Escape clears.

use bevy_ecs::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use bevy_log::prelude::*;
use bevy_ui::ui_transform::UiGlobalTransform;
use bevy_window::PrimaryWindow;
use jackdaw_api::prelude::*;
use jackdaw_jsn::{Brush, BrushFaceData, BrushGroup, BrushPlane};

use crate::brush::{
    BrushEditMode, BrushMeshCache, BrushSelection, ClipMode, ClipState, EditMode, SetBrush,
};
use crate::commands::{CommandGroup, CommandHistory};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::{CreateBrushCommand, brush_data_from_entity};
use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};
use crate::viewport_util::window_to_viewport_cursor_for;
use jackdaw_geometry::{
    EPSILON, compute_face_tangent_axes,
    halfedge::{
        HalfedgeMesh,
        ops::bisect_plane::{BisectKeep, bisect_plane},
    },
    point_inside_all_planes,
};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ClipPlacePointOp>()
        .register_operator::<ClipCycleModeOp>()
        .register_operator::<ClipApplyOp>()
        .register_operator::<ClipClearOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<ClipCycleModeOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            related!(
                Bindings
                    [IntoBindingBundle::into_binding_bundle(((KeyCode::Tab, Press::default()),))]
            ),
        ));
        world.spawn((
            Action::<ClipApplyOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            related!(
                Bindings
                    [IntoBindingBundle::into_binding_bundle(((KeyCode::Enter, Press::default()),))]
            ),
        ));
        world.spawn((
            Action::<ClipClearOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            related!(
                Bindings[IntoBindingBundle::into_binding_bundle(((
                    KeyCode::Escape,
                    Press::default()
                ),))]
            ),
        ));
    });
}

/// LMB in clip mode dispatches `brush.clip.place_point`. Mouse-button
/// gestures aren't expressible as BEI key bindings.
pub(crate) fn place_point_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !is_clip_mode_value(&edit_mode)
        || keybind_focus.is_typing()
        || clip_state.points.len() >= 3
    {
        return;
    }
    commands.queue(|world: &mut World| {
        let _ = world
            .operator(ClipPlacePointOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: false,
            })
            .call();
    });
}

fn is_clip_mode_value(edit_mode: &EditMode) -> bool {
    matches!(edit_mode, EditMode::BrushEdit(BrushEditMode::Clip))
}

fn is_clip_mode_open(
    edit_mode: &EditMode,
    keybind_focus: &crate::keybind_focus::KeybindFocus,
) -> bool {
    !keybind_focus.is_typing() && is_clip_mode_value(edit_mode)
}

fn can_place_point(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    brush_selection: Res<BrushSelection>,
    clip_state: Res<ClipState>,
) -> bool {
    is_clip_mode_open(&edit_mode, &keybind_focus)
        && brush_selection.entity.is_some()
        && clip_state.points.len() < 3
}

fn can_apply_or_cycle(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
) -> bool {
    is_clip_mode_open(&edit_mode, &keybind_focus) && clip_state.preview_plane.is_some()
}

fn can_clear(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
    active: ActiveModalQuery,
) -> bool {
    if active.is_modal_running() {
        return false;
    }
    is_clip_mode_open(&edit_mode, &keybind_focus)
        && (!clip_state.points.is_empty() || clip_state.mode != ClipMode::KeepFront)
}

#[operator(
    id = "brush.clip.place_point",
    label = "Place Clip Point",
    description = "Raycast the cursor against the selected brush, snap, and add the \
                   resulting local-space point to `ClipState`. Availability \
                   (`can_place_point`) requires clip mode, a selected brush, and \
                   fewer than three existing points.",
    is_available = can_place_point,
    allows_undo = false,
)]
pub(crate) fn clip_place_point(
    _: In<OperatorParameters>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    active: Res<ActiveViewport>,
    keyboard: Res<ButtonInput<KeyCode>>,
    brush_selection: Res<BrushSelection>,
    brushes: Query<&Brush>,
    brush_transforms: Query<&GlobalTransform>,
    brush_caches: Query<&BrushMeshCache>,
    snap_settings: Res<crate::snapping::SnapSettings>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush) = brushes.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(cache) = brush_caches.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Ok(window) = primary_window.single() else {
        return OperatorResult::Cancelled;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return OperatorResult::Cancelled;
    };
    let Some(camera_entity) = active.camera else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_entity) = active.ui_node else {
        return OperatorResult::Cancelled;
    };
    let Ok((camera, cam_tf)) = camera_query.get(camera_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(viewport_cursor) =
        window_to_viewport_cursor_for(cursor_pos, camera, viewport_entity, &viewport_query)
    else {
        return OperatorResult::Cancelled;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return OperatorResult::Cancelled;
    };

    let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
    let mut best_t = f32::MAX;
    let mut best_point = None;

    for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
        if polygon.len() < 3 {
            continue;
        }
        let face = &brush.faces[face_idx];
        let world_normal = brush_rot * face.plane.normal;
        let face_centroid: Vec3 =
            polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
        let world_centroid = brush_global.transform_point(face_centroid);

        let denom = world_normal.dot(*ray.direction);
        if denom.abs() < EPSILON {
            continue;
        }
        let t = (world_centroid - ray.origin).dot(world_normal) / denom;
        if t > 0.0 && t < best_t {
            let hit = ray.origin + *ray.direction * t;
            let local_hit = brush_rot.inverse() * (hit - brush_trans);
            if point_inside_all_planes(local_hit, &brush.faces) {
                best_t = t;
                best_point = Some(local_hit);
            }
        }
    }

    let Some(local_hit) = best_point else {
        return OperatorResult::Cancelled;
    };

    let world_point = brush_global.transform_point(local_hit);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let snapped = snap_settings.snap_translate_vec3_if(world_point, ctrl);
    let local_snapped = brush_rot.inverse() * (snapped - brush_trans);
    clip_state.points.push(local_snapped);
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.cycle_mode",
    label = "Cycle Clip Mode",
    description = "Cycle `ClipState.mode` through KeepFront -> KeepBack -> Split. \
                   Availability (`can_apply_or_cycle`) requires clip mode and a \
                   computed preview plane.",
    is_available = can_apply_or_cycle,
    allows_undo = false,
)]
pub(crate) fn clip_cycle_mode(
    _: In<OperatorParameters>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    clip_state.mode = match clip_state.mode {
        ClipMode::KeepFront => ClipMode::KeepBack,
        ClipMode::KeepBack => ClipMode::Split,
        ClipMode::Split => ClipMode::KeepFront,
    };
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.clear",
    label = "Clear Clip Points",
    description = "Reset `ClipState` to its default (no points, KeepFront mode). \
                   Availability (`can_clear`) requires clip mode with non-default \
                   state and no active modal.",
    is_available = can_clear,
    allows_undo = false,
)]
pub(crate) fn clip_clear(
    _: In<OperatorParameters>,
    mut clip_state: ResMut<ClipState>,
) -> OperatorResult {
    *clip_state = ClipState::default();
    OperatorResult::Finished
}

#[operator(
    id = "brush.clip.apply",
    label = "Apply Clip",
    description = "Apply the preview plane to the selected brush per the current \
                   `ClipState.mode` (KeepFront / KeepBack / Split). Availability \
                   (`can_apply_or_cycle`) requires clip mode and a computed \
                   preview plane.",
    is_available = can_apply_or_cycle,
    allows_undo = false,
)]
pub(crate) fn clip_apply(
    _: In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_transforms: Query<&GlobalTransform>,
    mut clip_state: ResMut<ClipState>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(brush_entity) = brush_selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let Some(plane) = clip_state.preview_plane.clone() else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Dispatch: brushes with populated topology (the common case after the
    // topology migration) take the HalfedgeMesh bisect_plane path. Brushes with
    // Brushes always carry populated topology; the plane-push fast path
    // below stays as a safety net for malformed legacy inputs the
    // migration system hasn't touched yet.
    let use_bisect = !brush.topology.polygons.is_empty();

    if use_bisect {
        match clip_state.mode {
            ClipMode::KeepFront => {
                let Some(new_brush) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
                    warn!("Clip: bisect failed; aborting");
                    return OperatorResult::Cancelled;
                };
                push_brush_command(
                    &mut history,
                    brush_entity,
                    &mut brush,
                    new_brush,
                    "Clip brush (keep front)",
                );
            }
            ClipMode::KeepBack => {
                let Some(new_brush) = bisect_brush(&brush, &plane, BisectKeep::Back) else {
                    warn!("Clip: bisect failed; aborting");
                    return OperatorResult::Cancelled;
                };
                push_brush_command(
                    &mut history,
                    brush_entity,
                    &mut brush,
                    new_brush,
                    "Clip brush (keep back)",
                );
            }
            ClipMode::Split => {
                let Some(front) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
                    warn!("Clip: split bisect (front) failed; aborting");
                    return OperatorResult::Cancelled;
                };
                let Some(back) = bisect_brush(&brush, &plane, BisectKeep::Back) else {
                    warn!("Clip: split bisect (back) failed; aborting");
                    return OperatorResult::Cancelled;
                };
                let old = brush.clone();
                *brush = front.clone();
                let set_cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: front,
                    label: "Clip brush (split - front)".to_string(),
                };
                queue_split_spawn(&mut commands, brush_entity, brush_global, set_cmd, back);
            }
        }
    } else {
        // Legacy convex fast path: push a half-space face plane.
        let clip_face = clip_face_from_plane(&plane);
        let flipped_face = clip_face_from_plane(&BrushPlane {
            normal: -plane.normal,
            distance: -plane.distance,
        });

        match clip_state.mode {
            ClipMode::KeepFront => {
                push_face_command(
                    &mut history,
                    brush_entity,
                    &mut brush,
                    clip_face,
                    "Clip brush (keep front)",
                );
            }
            ClipMode::KeepBack => {
                push_face_command(
                    &mut history,
                    brush_entity,
                    &mut brush,
                    flipped_face,
                    "Clip brush (keep back)",
                );
            }
            ClipMode::Split => {
                let old = brush.clone();
                let mut front = old.clone();
                front.faces.push(clip_face);
                let mut back = old.clone();
                back.faces.push(flipped_face);
                *brush = front.clone();

                let set_cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: front,
                    label: "Clip brush (split - front)".to_string(),
                };
                queue_split_spawn(&mut commands, brush_entity, brush_global, set_cmd, back);
            }
        }
    }

    *clip_state = ClipState::default();
    OperatorResult::Finished
}

/// Lift the brush's topology into an `HalfedgeMesh`, bisect it along `plane`,
/// and flatten back into a new `Brush`. Returns `None` if the bisect
/// produces no faces (degenerate input or plane misses the brush).
///
/// Cap face handling: `bisect_plane` emits one new face for the cut
/// boundary with a fresh `material_idx` (= max + 1). We grow the
/// brush.faces slot array to match the post-flatten face count, copy
/// the source face's `BrushFaceData` for each non-cap slot, and seed
/// the cap slot from the first surviving face (so checker tiling
/// inherits from the cut's neighbor) before zeroing UV axes so
/// `ensure_uv_axes` derives proper tangents from the cap's plane.
fn bisect_brush(brush: &Brush, plane: &BrushPlane, keep: BisectKeep) -> Option<Brush> {
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let result = bisect_plane(&mut mesh, plane, keep).ok()?;
    if mesh.face_count() == 0 {
        return None;
    }

    let new_topology = mesh.flatten_to_topology();
    if new_topology.polygons.is_empty() {
        return None;
    }

    // Build the slot -> source-material-idx map by walking the flattened
    // topology in slot order. `flatten_to_topology` sorts faces by
    // `material_idx`, so slot N's source material is the Nth smallest
    // `material_idx` in the mesh.
    let mut slot_to_src_material: Vec<u32> =
        mesh.faces.iter().map(|(_, f)| f.material_idx).collect();
    slot_to_src_material.sort();

    let cap_idx = result.cap_material_idx;
    // Template face for the cap slot: prefer the first surviving non-cap
    // face so the cap inherits the closest neighbor's material / UV
    // tiling. Fall back to the last face of the source brush, or default.
    let cap_template = brush
        .faces
        .first()
        .cloned()
        .or_else(|| brush.faces.last().cloned())
        .unwrap_or_default();

    let mut new_faces: Vec<BrushFaceData> = Vec::with_capacity(new_topology.polygons.len());
    for &src_material in &slot_to_src_material {
        if src_material == cap_idx {
            // Cap slot: seed from template, mark as cap, clear UV axes so
            // `ensure_uv_axes` derives a fresh tangent basis from the cut
            // plane's normal.
            let mut cap = cap_template.clone();
            cap.is_cap = true;
            cap.uv_u_axis = Vec3::ZERO;
            cap.uv_v_axis = Vec3::ZERO;
            new_faces.push(cap);
        } else {
            let src_idx = src_material as usize;
            let src = brush
                .faces
                .get(src_idx)
                .cloned()
                .or_else(|| brush.faces.last().cloned())
                .unwrap_or_default();
            new_faces.push(src);
        }
    }

    // Update each face's plane (normal, distance) from the new topology
    // and ensure UV axes are populated for the cap slot.
    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (idx, face_data) in new_faces.iter_mut().enumerate() {
        if idx >= new_topology.polygons.len() {
            continue;
        }
        let normal = new_topology.face_normal_with(&positions, idx);
        let v0_idx =
            new_topology.loops[new_topology.polygons[idx].loop_start as usize].vert as usize;
        let distance = positions[v0_idx].dot(normal);
        face_data.plane.normal = normal;
        face_data.plane.distance = distance;
        face_data.ensure_uv_axes();
    }

    Some(Brush {
        faces: new_faces,
        topology: new_topology,
    })
}

fn push_brush_command(
    history: &mut CommandHistory,
    entity: Entity,
    brush: &mut Brush,
    new_brush: Brush,
    label: &str,
) {
    let old = brush.clone();
    *brush = new_brush.clone();
    let cmd = SetBrush {
        entity,
        old,
        new: new_brush,
        label: label.to_string(),
    };
    history.push_executed(Box::new(cmd));
}

fn queue_split_spawn(
    commands: &mut Commands,
    brush_entity: Entity,
    brush_global: &GlobalTransform,
    set_cmd: SetBrush,
    back: Brush,
) {
    let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
    let spawn_transform = Transform {
        translation: brush_trans,
        rotation: brush_rot,
        scale: Vec3::ONE,
    };
    commands.queue(move |world: &mut World| {
        let parent_group = world
            .get::<ChildOf>(brush_entity)
            .map(|c| c.0)
            .filter(|&p| world.get::<BrushGroup>(p).is_some());
        let actual_transform = if parent_group.is_some() {
            *world.get::<Transform>(brush_entity).unwrap()
        } else {
            spawn_transform
        };

        let mut spawner = world.spawn((
            Name::new("Brush"),
            back,
            actual_transform,
            Visibility::default(),
        ));
        if let Some(parent) = parent_group {
            spawner.insert(ChildOf(parent));
        }
        let entity = spawner.id();
        crate::scene_io::register_entity_in_ast(world, entity);

        let create_cmd = CreateBrushCommand {
            data: brush_data_from_entity(world, entity),
        };
        let group = CommandGroup {
            commands: vec![Box::new(set_cmd), Box::new(create_cmd)],
            label: "Split brush".to_string(),
        };
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(group));
    });
}

fn clip_face_from_plane(plane: &BrushPlane) -> BrushFaceData {
    let (u, v) = compute_face_tangent_axes(plane.normal);
    BrushFaceData {
        plane: plane.clone(),
        uv_offset: Vec2::ZERO,
        uv_scale: Vec2::ONE,
        uv_rotation: 0.0,
        uv_u_axis: u,
        uv_v_axis: v,
        ..default()
    }
}

fn push_face_command(
    history: &mut CommandHistory,
    entity: Entity,
    brush: &mut Brush,
    face: BrushFaceData,
    label: &str,
) {
    let old = brush.clone();
    brush.faces.push(face);
    let cmd = SetBrush {
        entity,
        old,
        new: brush.clone(),
        label: label.to_string(),
    };
    history.push_executed(Box::new(cmd));
}
