//! Clip-tool operators. Replace the keybind/click branches in
//! `brush::interaction::handle_clip_mode`. The remaining clip-mode
//! work (recomputing the preview plane and drawing the gizmo
//! overlay) stays in `interaction.rs`.
//!
//! Default keybinds: LMB places a point, Tab cycles mode, Enter
//! applies, Escape clears.

use bevy::{prelude::*, ui::ui_transform::UiGlobalTransform};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_scene_types::{Brush, BrushFaceData, BrushPlane, unsorted_global};

use crate::brush::{BrushEditMode, BrushMeshCache, BrushSelection, ClipMode, ClipState, EditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};
use crate::viewport_util::window_to_viewport_cursor_for;
use jackdaw_geometry::{
    compute_face_tangent_axes,
    halfedge::{
        HalfedgeMesh,
        ops::bisect_plane::{BisectKeep, bisect_plane},
    },
};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ClipPlacePointOp>()
        .register_operator::<ClipCycleModeOp>()
        .register_operator::<ClipApplyOp>()
        .register_operator::<ClipClearOp>()
        .register_operator::<BrushClipOp>();

    ctx.bind_operator::<CoreExtensionInputContext, ClipCycleModeOp>([PresetInput::key("Tab")]);
    ctx.bind_operator::<CoreExtensionInputContext, ClipApplyOp>([PresetInput::key("Enter")]);
    ctx.bind_operator::<CoreExtensionInputContext, ClipClearOp>([PresetInput::key("Escape")]);
}

/// LMB in clip mode dispatches `brush.clip.place_point`. Mouse-button
/// gestures aren't expressible as key bindings.
pub(crate) fn place_point_invoke_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    clip_state: Res<ClipState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left)
        || !is_clip_mode_value(&edit_mode)
        || keybind_focus.keyboard_is_spoken_for()
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
    !keybind_focus.keyboard_is_spoken_for() && is_clip_mode_value(edit_mode)
}

fn can_place_point(
    edit_mode: Res<EditMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
    brush_selection: Res<BrushSelection>,
    clip_state: Res<ClipState>,
) -> bool {
    is_clip_mode_open(&edit_mode, &keybind_focus)
        && brush_selection.active_brush.is_some()
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
    cursor: crate::viewport::UiCursorPos,
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
) -> Option<OperatorResult> {
    let brush_entity = brush_selection.active_brush?;
    let brush_global = brush_transforms.get(brush_entity).ok()?;
    let brush = brushes.get(brush_entity).ok()?;
    let cache = brush_caches.get(brush_entity).ok()?;
    let cursor_pos = cursor.get()?;
    let camera_entity = active.camera?;
    let viewport_entity = active.ui_node?;
    let (camera, cam_tf) = camera_query.get(camera_entity).ok()?;
    let viewport_cursor =
        window_to_viewport_cursor_for(cursor_pos, camera, viewport_entity, &viewport_query)?;
    let ray = camera.viewport_to_world(cam_tf, viewport_cursor).ok()?;

    // Pick in brush-local space: unwrap the ray into the brush's local frame
    // (rotation + translation only, matching the brush's rigid placement), then
    // ask jackdaw_pick which face the local ray hits. `cache.vertices` and the
    // face planes are already local, so the returned hit is the local point.
    let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
    let inv_rot = brush_rot.inverse();
    let local_origin = inv_rot * (ray.origin - brush_trans);
    let local_dir = inv_rot * *ray.direction;
    let faces: Vec<jackdaw_pick::Face<'_>> = cache
        .face_polygons
        .iter()
        .zip(brush.faces.iter())
        .map(|(ring, face)| jackdaw_pick::Face {
            ring,
            plane: face.plane.clone(),
        })
        .collect();
    let (_, local_hit) =
        jackdaw_pick::face_from_ray(local_origin, local_dir, &cache.vertices, &faces)?;

    let world_point = brush_global.transform_point(local_hit);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let snapped = snap_settings.snap_translate_vec3_if(world_point, ctrl);
    let local_snapped = brush_rot.inverse() * (snapped - brush_trans);
    clip_state.points.push(local_snapped);
    Some(OperatorResult::Finished)
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
)]
pub(crate) fn clip_apply(
    _: In<OperatorParameters>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_transforms: Query<&GlobalTransform>,
    mut clip_state: ResMut<ClipState>,
    mut commands: Commands,
) -> Option<OperatorResult> {
    let brush_entity = brush_selection.active_brush?;
    let mut brush = brushes.get_mut(brush_entity).ok()?;
    let plane = clip_state.preview_plane.clone()?;
    let brush_global = brush_transforms.get(brush_entity).ok()?;

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
                    return None;
                };
                apply_clip_geometry(&mut commands, brush_entity, &mut brush, new_brush);
            }
            ClipMode::KeepBack => {
                let Some(new_brush) = bisect_brush(&brush, &plane, BisectKeep::Back) else {
                    warn!("Clip: bisect failed; aborting");
                    return None;
                };
                apply_clip_geometry(&mut commands, brush_entity, &mut brush, new_brush);
            }
            ClipMode::Split => {
                let Some(front) = bisect_brush(&brush, &plane, BisectKeep::Front) else {
                    warn!("Clip: split bisect (front) failed; aborting");
                    return None;
                };
                let Some(back) = bisect_brush(&brush, &plane, BisectKeep::Back) else {
                    warn!("Clip: split bisect (back) failed; aborting");
                    return None;
                };
                *brush = front.clone();
                queue_split_spawn(&mut commands, brush_entity, brush_global, front, back);
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
                apply_clip_face(&mut commands, brush_entity, &mut brush, clip_face);
            }
            ClipMode::KeepBack => {
                apply_clip_face(&mut commands, brush_entity, &mut brush, flipped_face);
            }
            ClipMode::Split => {
                let old = brush.clone();
                let mut front = old.clone();
                front.faces.push(clip_face);
                let mut back = old.clone();
                back.faces.push(flipped_face);
                *brush = front.clone();
                queue_split_spawn(&mut commands, brush_entity, brush_global, front, back);
            }
        }
    }

    *clip_state = ClipState::default();
    Some(OperatorResult::Finished)
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
pub(crate) fn bisect_brush(brush: &Brush, plane: &BrushPlane, keep: BisectKeep) -> Option<Brush> {
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

fn apply_clip_geometry(
    commands: &mut Commands,
    entity: Entity,
    brush: &mut Brush,
    new_brush: Brush,
) {
    *brush = new_brush.clone();
    commands.queue(move |world: &mut World| {
        crate::brush::sync_brush_to_ast(world, entity, &new_brush);
    });
}

fn queue_split_spawn(
    commands: &mut Commands,
    brush_entity: Entity,
    brush_global: &GlobalTransform,
    front: Brush,
    back: Brush,
) {
    let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
    let spawn_transform = Transform {
        translation: brush_trans,
        rotation: brush_rot,
        scale: Vec3::ONE,
    };
    commands.queue(move |world: &mut World| {
        let parent = world.get::<ChildOf>(brush_entity).map(|c| c.0);
        let actual_transform = if parent.is_some() {
            world
                .get::<Transform>(brush_entity)
                .copied()
                .unwrap_or(spawn_transform)
        } else {
            spawn_transform
        };
        if crate::entity_ops::spawn_cloned_brush_with_geometry(
            world,
            brush_entity,
            back,
            actual_transform,
        )
        .is_none()
        {
            warn!("Clip split spawn failed: source {brush_entity:?} has no document node");
        }
        crate::brush::sync_brush_to_ast(world, brush_entity, &front);
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

fn apply_clip_face(
    commands: &mut Commands,
    entity: Entity,
    brush: &mut Brush,
    face: BrushFaceData,
) {
    brush.faces.push(face);
    let new_brush = brush.clone();
    commands.queue(move |world: &mut World| {
        crate::brush::sync_brush_to_ast(world, entity, &new_brush);
    });
}

/// Which side of a plane `brush.clip` keeps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClipKeep {
    Front,
    Back,
    Both,
}

/// A `brush.clip` call, checked.
struct PlaneClip {
    entity: Entity,
    /// The plane in the brush's own space.
    plane: BrushPlane,
    keep: ClipKeep,
}

impl PlaneClip {
    fn read(world: &World, params: &OperatorParameters) -> Result<Self, String> {
        if !params.contains_key("entity") {
            return Err("name the brush to clip with `entity`".to_string());
        }
        let entity = crate::draw_brush::operand_node(world, params, "entity")?;
        if world.get::<Brush>(entity).is_none() {
            return Err(format!("`entity` {entity} is not a brush"));
        }
        let normal = params
            .as_vec3("normal")
            .ok_or("give the plane's world-space `normal` as x,y,z")?;
        if !normal.is_finite() || normal.length_squared() < 1e-12 {
            return Err(format!("`normal` {normal} has no direction"));
        }
        let point = params
            .as_vec3("point")
            .ok_or("give a world-space `point` on the plane as x,y,z")?;
        if !point.is_finite() {
            return Err(format!("`point` {point} is not a place"));
        }
        let keep = if params.contains_key("keep") {
            match params.as_str("keep") {
                Some("front") => ClipKeep::Front,
                Some("back") => ClipKeep::Back,
                Some("both") => ClipKeep::Both,
                _ => return Err("`keep` must be `front`, `back` or `both`".to_string()),
            }
        } else {
            ClipKeep::Front
        };
        let global = unsorted_global(world, entity)
            .ok_or_else(|| format!("`entity` {entity} has no transform"))?;
        let affine = global.affine();
        let local_normal = (Mat3::from(affine.matrix3).transpose() * normal).normalize_or_zero();
        if local_normal == Vec3::ZERO {
            return Err(format!("`entity` {entity} is scaled flat"));
        }
        let local_point = affine.inverse().transform_point3(point);
        Ok(Self {
            entity,
            plane: BrushPlane {
                normal: local_normal,
                distance: local_normal.dot(local_point),
            },
            keep,
        })
    }
}

/// End a clip that leaves the scene as it was, telling the caller why.
fn refuse_clip(world: &mut World, outcome: Outcome, why: impl AsRef<str>) -> OperatorResult {
    warn_caller(world, format!("brush.clip: {}", why.as_ref()));
    set_outcome(world, outcome);
    OperatorResult::Cancelled
}

#[operator(
    id = "brush.clip",
    label = "Clip Brush by Plane",
    description = "Clip a brush by a plane given as a world-space normal and a point on it, \
                   without the clip tool. The side the normal points to is the front. `keep = \
                   both` keeps the front in the brush and adds the back as a new brush named \
                   `<brush> 2`. A plane that does not cross the brush changes nothing.",
    params(
        entity(Entity, doc = "Brush to clip."),
        normal(Vec3, doc = "World-space normal of the plane."),
        point(Vec3, doc = "A world-space point on the plane."),
        keep(String, doc = "`front`, `back` or `both`. Defaults to `front`."),
    )
)]
pub(crate) fn brush_clip(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let clip = match PlaneClip::read(world, &params) {
        Ok(clip) => clip,
        Err(why) => return refuse_clip(world, Outcome::Invalid, why),
    };
    let Some(brush) = crate::draw_brush::evaluated_brush(world, clip.entity) else {
        return refuse_clip(world, Outcome::Invalid, "the brush went away");
    };

    let eps = crate::draw_brush::plane_distance_eps(&brush.topology);
    let (mut front, mut back) = (false, false);
    for vertex in &brush.topology.vertices {
        let side = clip.plane.normal.dot(vertex.position) - clip.plane.distance;
        front |= side > eps;
        back |= side < -eps;
    }
    if !(front && back) {
        return refuse_clip(
            world,
            Outcome::Unchanged,
            "the plane does not cross the brush, so nothing was clipped",
        );
    }

    let failed = "the plane cut failed, so nothing was clipped";
    let kept_side = if clip.keep == ClipKeep::Back {
        BisectKeep::Back
    } else {
        BisectKeep::Front
    };
    let Some(kept) = bisect_brush(&brush, &clip.plane, kept_side) else {
        return refuse_clip(world, Outcome::Unchanged, failed);
    };
    let split = if clip.keep == ClipKeep::Both {
        let Some(back) = bisect_brush(&brush, &clip.plane, BisectKeep::Back) else {
            return refuse_clip(world, Outcome::Unchanged, failed);
        };
        Some(back)
    } else {
        None
    };

    // Baked before the back half is cloned from the brush, so neither half
    // carries the stack that is already folded into it.
    crate::brush::topology_ops::mirror_ops::bake_mirror(world, clip.entity);
    let mut created = Vec::new();
    if let Some(back) = split {
        let local = world
            .get::<Transform>(clip.entity)
            .copied()
            .unwrap_or_default();
        match crate::entity_ops::spawn_cloned_brush_with_geometry(world, clip.entity, back, local) {
            Some(spawned) => {
                let base = crate::draw_brush::name_or_default(world, clip.entity);
                let mut taken = crate::draw_brush::names_besides(world, spawned);
                let name = crate::draw_brush::free_name(&mut taken, &format!("{base}2"));
                crate::commands::rename_scene_node(world, spawned, &name);
                crate::commands::SpawnedEntities::record(world, spawned);
                created.push(spawned);
            }
            None => warn_caller(
                world,
                "brush.clip: the back half could not be added, so only the front is kept",
            ),
        }
    }
    crate::brush::replace_brush(world, clip.entity, &kept);
    jackdaw_api_internal::operator::report_replaced(world, &[], &created);
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}
