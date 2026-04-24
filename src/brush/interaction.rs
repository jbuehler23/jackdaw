use std::collections::HashSet;

use bevy::{ecs::system::SystemParam, input_focus::InputFocus, prelude::*};

use crate::default_style;
use crate::{
    commands::CommandHistory,
    draw_brush::{CreateBrushCommand, brush_data_from_entity},
    keybinds::{EditorAction, KeybindRegistry},
    selection::{Selected, Selection},
    viewport::{MainViewportCamera, SceneViewport},
    viewport_util::{point_in_polygon_2d, point_to_segment_dist, window_to_viewport_cursor},
};

const MIN_EXTRUDE_DEPTH: f32 = 0.01;

use super::hull::rebuild_brush_from_vertices;
use super::{BrushEditMode, BrushMeshCache, BrushSelection, EditMode, SetBrush};
use jackdaw_geometry::{
    EPSILON, brush_planes_to_world, compute_brush_geometry, compute_face_tangent_axes,
    point_inside_all_planes,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushGroup, BrushPlane};

/// Bundled keyboard + keybind input to keep system parameter counts under the 16-param limit.
#[derive(SystemParam)]
pub(super) struct KeyboardInput<'w> {
    pub keyboard: Res<'w, ButtonInput<KeyCode>>,
    pub keybinds: Res<'w, KeybindRegistry>,
}

/// Reactive cleanup: when the active brush entity is no longer
/// selected, drop out of brush-edit mode. Also the single remaining
/// keybind for this subsystem — Escape exits brush-edit back to
/// Object, deferring to clip mode's own handler if points are pending.
/// The digit-key mode switches (1/2/3/4) moved to
/// [`crate::edit_mode_ops`]; the toolbar buttons there dispatch the
/// same operators.
pub(super) fn handle_edit_mode_keys(
    input_focus: Res<InputFocus>,
    input: KeyboardInput,
    selection: Res<Selection>,
    mut edit_mode: ResMut<EditMode>,
    mut brush_selection: ResMut<BrushSelection>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    face_drag: Res<BrushDragState>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    clip_state: Res<ClipState>,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;
    if input_focus.0.is_some() || modal.active.is_some() {
        return;
    }

    // Exit brush edit mode if the brush entity gets deselected
    if let EditMode::BrushEdit(_) = *edit_mode
        && let Some(brush_entity) = brush_selection.entity
        && selection.primary() != Some(brush_entity)
    {
        // Save last selected face for extend-to-brush fallback
        if !brush_selection.faces.is_empty() {
            brush_selection.last_face_entity = Some(brush_entity);
            brush_selection.last_face_index = brush_selection.faces.last().copied();
        }
        *edit_mode = EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }

    // Don't switch modes while any drag is active
    if face_drag.active || vertex_drag.active || edge_drag.active {
        return;
    }
    if face_drag.pending.is_some() || vertex_drag.pending.is_some() || edge_drag.pending.is_some() {
        return;
    }

    // Escape: exit to Object (unless Clip mode with pending points)
    if keybinds.just_pressed(EditorAction::ExitEditMode, keyboard) {
        if let EditMode::BrushEdit(BrushEditMode::Clip) = *edit_mode
            && !clip_state.points.is_empty()
        {
            // Let clip mode's own Escape handler clear the points first
            return;
        }
        if matches!(*edit_mode, EditMode::BrushEdit(_)) {
            *edit_mode = EditMode::Object;
            brush_selection.entity = None;
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
        }
    }
}

pub(super) fn brush_face_interact(
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    input: KeyboardInput,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    face_entities: Query<(Entity, &super::BrushFaceEntity, &GlobalTransform)>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    selection: Res<Selection>,
    mut brushes: Query<(&mut Brush, &GlobalTransform)>,
    mut drag_state: ResMut<BrushDragState>,
    input_focus: Res<InputFocus>,
    mut history: ResMut<CommandHistory>,
    mut commands: Commands,
    snap_settings: Res<crate::snapping::SnapSettings>,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;
    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));

    // PageUp/PageDown: nudge selected face vertices vertically (gabling)
    if in_face_edit
        && !drag_state.active
        && drag_state.pending.is_none()
        && !brush_selection.faces.is_empty()
        && let Some(brush_entity) = brush_selection.entity
    {
        let nudge_dir = if keybinds.key_just_pressed(EditorAction::NudgeUp, keyboard) {
            Some(1.0)
        } else if keybinds.key_just_pressed(EditorAction::NudgeDown, keyboard) {
            Some(-1.0)
        } else {
            None
        };
        if let Some(dir) = nudge_dir {
            let grid = snap_settings.grid_size();
            if let Ok(cache) = brush_caches.get(brush_entity)
                && let Ok((mut brush, _)) = brushes.get_mut(brush_entity)
            {
                let old = brush.clone();
                let offset = Vec3::new(0.0, dir * grid, 0.0);
                let mut new_verts = cache.vertices.clone();
                // Collect unique vertex indices from all selected faces
                let mut affected: HashSet<usize> = HashSet::new();
                for &fi in &brush_selection.faces {
                    if let Some(poly) = cache.face_polygons.get(fi) {
                        affected.extend(poly.iter().copied());
                    }
                }
                for vi in &affected {
                    if *vi < new_verts.len() {
                        new_verts[*vi] += offset;
                    }
                }
                if let Some((new_brush, old_to_new)) = rebuild_brush_from_vertices(
                    &old,
                    &cache.vertices,
                    &cache.face_polygons,
                    &new_verts,
                ) {
                    *brush = new_brush;
                    // Remap face selection to match new face ordering
                    brush_selection.faces = brush_selection
                        .faces
                        .iter()
                        .filter_map(|&fi| old_to_new.get(fi).copied())
                        .collect();
                    let cmd = SetBrush {
                        entity: brush_entity,
                        old,
                        new: brush.clone(),
                        label: "Nudge brush face".to_string(),
                    };
                    history.push_executed(Box::new(cmd));
                }
            }
            return;
        }
    }

    if !in_face_edit && drag_state.pending.is_none() && !drag_state.active {
        // Not in face mode. Shift+click or Alt+click enters quick face edit
        let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        if !(shift || alt) || !mouse.just_pressed(MouseButton::Left) {
            return;
        }
        // Fall through to face picking below
    }

    if input_focus.0.is_some() && !in_face_edit {
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // Cancel active drag on Escape or right-click
    if drag_state.active
        && (keybinds.just_pressed(EditorAction::ExitEditMode, keyboard)
            || mouse.just_pressed(MouseButton::Right))
    {
        match drag_state.extrude_mode {
            FaceExtrudeMode::Merge => {
                // Revert brush to start state
                if let Some(brush_entity) = brush_selection.entity
                    && let Some(ref start) = drag_state.start_brush
                    && let Ok((mut brush, _)) = brushes.get_mut(brush_entity)
                {
                    *brush = start.clone();
                }
            }
            FaceExtrudeMode::Extend => {
                // Original brush was never modified, just clear state
            }
        }
        drag_state.active = false;
        drag_state.pending = None;
        drag_state.extend_face_polygon.clear();
        drag_state.extend_depth = 0.0;
        if drag_state.quick_action {
            *edit_mode = EditMode::Object;
            brush_selection.entity = None;
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
            drag_state.quick_action = false;
        }
        return;
    }

    // Release: commit drag
    if mouse.just_released(MouseButton::Left) {
        if drag_state.active {
            match drag_state.extrude_mode {
                FaceExtrudeMode::Merge => {
                    if let Some(brush_entity) = brush_selection.entity
                        && let Some(ref start) = drag_state.start_brush
                        && let Ok((brush, _)) = brushes.get(brush_entity)
                    {
                        let cmd = SetBrush {
                            entity: brush_entity,
                            old: start.clone(),
                            new: brush.clone(),
                            label: "Move brush face".to_string(),
                        };
                        history.push_executed(Box::new(cmd));
                    }
                }
                FaceExtrudeMode::Extend => {
                    if drag_state.extend_depth.abs() > MIN_EXTRUDE_DEPTH {
                        spawn_extruded_brush(
                            &drag_state.extend_face_polygon,
                            drag_state.extend_face_normal,
                            drag_state.extend_depth,
                            &mut commands,
                        );
                    }
                }
            }
            drag_state.active = false;
            drag_state.extend_face_polygon.clear();
            drag_state.extend_depth = 0.0;
        }
        let was_quick = drag_state.quick_action;
        drag_state.pending = None;
        if was_quick {
            *edit_mode = EditMode::Object;
            brush_selection.entity = None;
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
            drag_state.quick_action = false;
        }
        return;
    }

    // Pending → active promotion (5px threshold)
    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
    {
        let dist = (cursor_pos - pending.click_pos).length();
        if dist > 5.0 {
            // Promote to active drag
            if let Some(brush_entity) = brush_selection.entity
                && let Ok((brush, brush_global)) = brushes.get(brush_entity)
            {
                drag_state.active = true;
                drag_state.start_cursor = viewport_cursor;
                // Use the first selected face's normal
                if let Some(&face_idx) = brush_selection.faces.first()
                    && face_idx < brush.faces.len()
                {
                    drag_state.drag_face_normal = brush.faces[face_idx].plane.normal;
                }

                match drag_state.extrude_mode {
                    FaceExtrudeMode::Merge => {
                        drag_state.start_brush = Some(brush.clone());
                    }
                    FaceExtrudeMode::Extend => {
                        // Capture world-space face polygon vertices for preview
                        let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
                        drag_state.extend_face_normal =
                            (brush_rot * drag_state.drag_face_normal).normalize();
                        if let Ok(cache) = brush_caches.get(brush_entity)
                            && let Some(&face_idx) = brush_selection.faces.first()
                        {
                            let polygon = &cache.face_polygons[face_idx];
                            drag_state.extend_face_polygon = polygon
                                .iter()
                                .map(|&vi| brush_global.transform_point(cache.vertices[vi]))
                                .collect();
                        }
                        drag_state.extend_depth = 0.0;
                    }
                }
            }
        }
    }

    // Continue active drag
    if drag_state.active {
        let Some(brush_entity) = brush_selection.entity else {
            drag_state.active = false;
            return;
        };

        match drag_state.extrude_mode {
            FaceExtrudeMode::Merge => {
                // Adjust face plane distance (push/pull)
                let Ok((mut brush, brush_global)) = brushes.get_mut(brush_entity) else {
                    drag_state.active = false;
                    return;
                };
                let Some(ref start) = drag_state.start_brush else {
                    drag_state.active = false;
                    return;
                };

                let brush_pos = brush_global.translation();
                let Ok(origin_screen) = camera.world_to_viewport(cam_tf, brush_pos) else {
                    return;
                };
                let Ok(normal_screen) =
                    camera.world_to_viewport(cam_tf, brush_pos + drag_state.drag_face_normal)
                else {
                    return;
                };
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - drag_state.start_cursor;
                let projected = mouse_delta.dot(screen_dir);

                let cam_dist = (cam_tf.translation() - brush_pos).length();
                let drag_amount = projected * cam_dist * 0.003;
                let drag_amount = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (drag_amount / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    drag_amount
                };

                for &face_idx in &brush_selection.faces {
                    if face_idx < start.faces.len() && face_idx < brush.faces.len() {
                        brush.faces[face_idx].plane.distance =
                            start.faces[face_idx].plane.distance + drag_amount;
                    }
                }
            }
            FaceExtrudeMode::Extend => {
                // Compute extend depth from mouse projection, don't modify original brush
                if drag_state.extend_face_polygon.is_empty() {
                    drag_state.active = false;
                    return;
                }

                let face_centroid: Vec3 = drag_state.extend_face_polygon.iter().sum::<Vec3>()
                    / drag_state.extend_face_polygon.len() as f32;
                let world_normal = drag_state.extend_face_normal;

                let Ok(origin_screen) = camera.world_to_viewport(cam_tf, face_centroid) else {
                    return;
                };
                let Ok(normal_screen) =
                    camera.world_to_viewport(cam_tf, face_centroid + world_normal)
                else {
                    return;
                };
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - drag_state.start_cursor;
                let projected = mouse_delta.dot(screen_dir);

                let cam_dist = (cam_tf.translation() - face_centroid).length();
                let raw_depth = projected * cam_dist * 0.003;
                drag_state.extend_depth = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (raw_depth / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    raw_depth
                };
            }
        }
        return;
    }

    // Mouse press: pick face and start pending drag
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    // Determine which brush entity to work with
    let brush_entity = if in_face_edit {
        brush_selection.entity
    } else {
        selection.primary().filter(|&e| brushes.contains(e))
    };
    let Some(brush_entity) = brush_entity else {
        return;
    };

    let Ok(cache) = brush_caches.get(brush_entity) else {
        return;
    };

    // Find face whose screen-space polygon contains the cursor.
    // When multiple faces overlap (e.g. back-face behind front-face),
    // pick the one whose centroid is closest to the camera.
    let mut best_face = None;
    let mut best_depth = f32::MAX;

    for (_, face_ent, face_global) in &face_entities {
        if face_ent.brush_entity != brush_entity {
            continue;
        }
        let face_idx = face_ent.face_index;
        let polygon = &cache.face_polygons[face_idx];
        if polygon.len() < 3 {
            continue;
        }

        let brush_tf = face_global;

        // Project face polygon vertices to screen space
        let screen_verts: Vec<Vec2> = polygon
            .iter()
            .filter_map(|&vi| {
                let world = brush_tf.transform_point(cache.vertices[vi]);
                camera.world_to_viewport(cam_tf, world).ok()
            })
            .collect();
        if screen_verts.len() < 3 {
            continue;
        }

        if point_in_polygon_2d(viewport_cursor, &screen_verts) {
            // Use depth of centroid to resolve overlapping faces
            let centroid: Vec3 =
                polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
            let world_centroid = brush_tf.transform_point(centroid);
            let depth = (cam_tf.translation() - world_centroid).length_squared();
            if depth < best_depth {
                best_depth = depth;
                best_face = Some(face_idx);
            }
        }
    }

    if let Some(face_idx) = best_face {
        // Auto-enter face edit mode if not already in it
        if !in_face_edit {
            *edit_mode = EditMode::BrushEdit(BrushEditMode::Face);
            brush_selection.entity = Some(brush_entity);
            brush_selection.faces.clear();
            brush_selection.vertices.clear();
            brush_selection.edges.clear();
            drag_state.quick_action = true;
        }

        if in_face_edit && ctrl {
            // Ctrl+click in face mode: toggle multi-select (no drag)
            if let Some(pos) = brush_selection.faces.iter().position(|&f| f == face_idx) {
                brush_selection.faces.remove(pos);
            } else {
                brush_selection.faces.push(face_idx);
            }
        } else {
            brush_selection.faces = vec![face_idx];
            // Determine extrude mode:
            // - From Object mode: Shift = Merge (push/pull), Alt = Extend (new brush)
            // - In face mode: plain drag = Merge
            if !in_face_edit {
                drag_state.extrude_mode = if alt {
                    FaceExtrudeMode::Extend
                } else {
                    FaceExtrudeMode::Merge
                };
            } else {
                drag_state.extrude_mode = FaceExtrudeMode::Merge;
                drag_state.quick_action = false;
            }
            // Record pending drag
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
        }
    } else if in_face_edit && !ctrl {
        // Click outside any face: exit to Object mode
        *edit_mode = EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }
}

fn spawn_extruded_brush(
    face_polygon_world: &[Vec3],
    world_normal: Vec3,
    depth: f32,
    commands: &mut Commands,
) {
    if face_polygon_world.len() < 3 || depth.abs() < MIN_EXTRUDE_DEPTH {
        return;
    }

    let face_polygon = face_polygon_world.to_vec();
    let normal = world_normal;

    commands.queue(move |world: &mut World| {
        // Compute volume center = face centroid + normal * depth/2
        let face_centroid: Vec3 = face_polygon.iter().sum::<Vec3>() / face_polygon.len() as f32;
        let center = face_centroid + normal * depth / 2.0;

        // Build rotation: local Y = face normal (same pattern as spawn_drawn_brush)
        let rotation = if normal == Vec3::Y {
            Quat::IDENTITY
        } else if normal == Vec3::NEG_Y {
            Quat::from_rotation_x(std::f32::consts::PI)
        } else {
            let (u, _v) = compute_face_tangent_axes(normal);
            let target_mat = Mat3::from_cols(u, normal, -normal.cross(u).normalize());
            Quat::from_mat3(&target_mat)
        };
        let inv_rotation = rotation.inverse();

        // Convert polygon vertices to local space (centered at `center`)
        let local_verts: Vec<Vec3> = face_polygon
            .iter()
            .map(|&v| inv_rotation * (v - center))
            .collect();

        let Some(mut brush) = Brush::prism(&local_verts, Vec3::Y, depth) else {
            return;
        };

        // Apply last-used material
        let last_mat = world.resource::<super::LastUsedMaterial>().material.clone();
        if let Some(ref mat) = last_mat {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }

        let entity = world
            .spawn((
                Name::new("Brush"),
                brush,
                Transform {
                    translation: center,
                    rotation,
                    scale: Vec3::ONE,
                },
                Visibility::default(),
            ))
            .id();

        // Select the new brush
        {
            let selection = world.resource::<Selection>();
            let old_selected: Vec<Entity> = selection.entities.clone();
            for &e in &old_selected {
                if let Ok(mut ec) = world.get_entity_mut(e) {
                    ec.remove::<Selected>();
                }
            }
            let mut selection = world.resource_mut::<Selection>();
            selection.entities = vec![entity];
            world.entity_mut(entity).insert(Selected);
        }

        // Store brush data for undo
        let cmd = CreateBrushCommand {
            data: brush_data_from_entity(world, entity),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(cmd));
    });
}

pub(super) fn brush_vertex_interact(
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    input: KeyboardInput,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<VertexDragState>,
    input_focus: Res<InputFocus>,
    mut history: ResMut<CommandHistory>,
    snap_settings: Res<crate::snapping::SnapSettings>,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;
    let EditMode::BrushEdit(BrushEditMode::Vertex) = *edit_mode else {
        drag_state.active = false;
        drag_state.pending = None;
        return;
    };
    if input_focus.0.is_some() {
        return;
    }

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };

    // PageUp/PageDown: nudge selected vertices vertically (no cursor needed)
    if !drag_state.active && drag_state.pending.is_none() && !brush_selection.vertices.is_empty() {
        let nudge_dir = if keybinds.key_just_pressed(EditorAction::NudgeUp, keyboard) {
            Some(1.0)
        } else if keybinds.key_just_pressed(EditorAction::NudgeDown, keyboard) {
            Some(-1.0)
        } else {
            None
        };
        if let Some(dir) = nudge_dir {
            let grid = snap_settings.grid_size();
            if let Ok(cache) = brush_caches.get(brush_entity)
                && let Ok(mut brush) = brushes.get_mut(brush_entity)
            {
                let old = brush.clone();
                let offset = Vec3::new(0.0, dir * grid, 0.0);
                let mut new_verts = cache.vertices.clone();
                for &vi in &brush_selection.vertices {
                    if vi < new_verts.len() {
                        new_verts[vi] += offset;
                    }
                }
                if let Some((new_brush, _)) = rebuild_brush_from_vertices(
                    &old,
                    &cache.vertices,
                    &cache.face_polygons,
                    &new_verts,
                ) {
                    *brush = new_brush;
                    let cmd = SetBrush {
                        entity: brush_entity,
                        old,
                        new: brush.clone(),
                        label: "Nudge brush vertex".to_string(),
                    };
                    history.push_executed(Box::new(cmd));
                }
            }
            return;
        }
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    // Axis constraint toggle during active drag
    if drag_state.active {
        if keybinds.just_pressed(EditorAction::ConstrainX, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisX {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisX
            };
        } else if keybinds.just_pressed(EditorAction::ConstrainY, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisY {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisY
            };
        } else if keybinds.just_pressed(EditorAction::ConstrainZ, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisZ {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisZ
            };
        }
    }

    // Cancel active drag on Escape or right-click
    if drag_state.active
        && (keybinds.just_pressed(EditorAction::ExitEditMode, keyboard)
            || mouse.just_pressed(MouseButton::Right))
    {
        if let Some(ref start) = drag_state.start_brush
            && let Ok(mut brush) = brushes.get_mut(brush_entity)
        {
            *brush = start.clone();
        }
        drag_state.active = false;
        drag_state.pending = None;
        drag_state.constraint = VertexDragConstraint::Free;
        drag_state.split_vertex = None;
        return;
    }

    // Release: commit drag
    if mouse.just_released(MouseButton::Left) {
        if drag_state.active {
            if let Some(ref start) = drag_state.start_brush
                && let Ok(brush) = brushes.get(brush_entity)
            {
                let label = if drag_state.split_vertex.is_some() {
                    "Split brush vertex"
                } else {
                    "Move brush vertex"
                };
                let cmd = SetBrush {
                    entity: brush_entity,
                    old: start.clone(),
                    new: brush.clone(),
                    label: label.to_string(),
                };
                history.push_executed(Box::new(cmd));
            }
            drag_state.active = false;
            drag_state.constraint = VertexDragConstraint::Free;
        }
        drag_state.pending = None;
        drag_state.split_vertex = None;
        return;
    }

    // Pending → active promotion (5px threshold)
    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
    {
        let dist = (cursor_pos - pending.click_pos).length();
        if dist > 5.0
            && let Ok(cache) = brush_caches.get(brush_entity)
            && let Ok(brush) = brushes.get(brush_entity)
        {
            drag_state.active = true;
            drag_state.constraint = VertexDragConstraint::Free;
            drag_state.start_brush = Some(brush.clone());
            drag_state.start_cursor = viewport_cursor;

            // Build start vertices, possibly with split vertex appended
            let mut all_verts = cache.vertices.clone();
            if let Some(split_pos) = drag_state.split_vertex {
                all_verts.push(split_pos);
            }

            drag_state.start_vertex_positions = brush_selection
                .vertices
                .iter()
                .map(|&vi| all_verts.get(vi).copied().unwrap_or(Vec3::ZERO))
                .collect();
            drag_state.start_all_vertices = all_verts;
            drag_state.start_face_polygons = cache.face_polygons.clone();
        }
    }

    // Continue active drag
    if drag_state.active {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            drag_state.active = false;
            return;
        };
        let Some(ref start) = drag_state.start_brush else {
            drag_state.active = false;
            return;
        };
        let Ok(brush_global) = brush_transforms.get(brush_entity) else {
            return;
        };

        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let Some(local_offset) = compute_brush_drag_offset(
            drag_state.constraint,
            mouse_delta,
            cam_tf,
            camera,
            brush_global,
        ) else {
            return;
        };

        let mut new_verts = drag_state.start_all_vertices.clone();
        for (sel_idx, &vert_idx) in brush_selection.vertices.iter().enumerate() {
            if sel_idx < drag_state.start_vertex_positions.len() && vert_idx < new_verts.len() {
                new_verts[vert_idx] = drag_state.start_vertex_positions[sel_idx] + local_offset;
            }
        }

        if let Some((new_brush, _)) = rebuild_brush_from_vertices(
            start,
            &drag_state.start_all_vertices,
            &drag_state.start_face_polygons,
            &new_verts,
        ) {
            *brush = new_brush;
        }
        return;
    }

    // Mouse press: pick vertex and start pending drag
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(cache) = brush_caches.get(brush_entity) else {
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return;
    };

    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    // Shift+click: pick edge midpoint or face center for vertex split
    if shift && !ctrl {
        // Collect unique edges from face polygons
        let mut unique_edges: Vec<(usize, usize)> = Vec::new();
        for polygon in &cache.face_polygons {
            if polygon.len() < 2 {
                continue;
            }
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                let edge = (a.min(b), a.max(b));
                if !unique_edges.contains(&edge) {
                    unique_edges.push(edge);
                }
            }
        }

        let mut best_split: Option<Vec3> = None;
        let mut best_dist = 20.0_f32;

        // Try edge midpoints first
        for &(a, b) in &unique_edges {
            let midpoint = (cache.vertices[a] + cache.vertices[b]) * 0.5;
            let world_pos = brush_global.transform_point(midpoint);
            if let Ok(screen_pos) = camera.world_to_viewport(cam_tf, world_pos) {
                let dist = (screen_pos - viewport_cursor).length();
                if dist < best_dist {
                    best_dist = dist;
                    best_split = Some(midpoint);
                }
            }
        }

        // Fallback: face centers
        if best_split.is_none() {
            best_dist = 20.0;
            for polygon in &cache.face_polygons {
                if polygon.len() < 3 {
                    continue;
                }
                let centroid: Vec3 = polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>()
                    / polygon.len() as f32;
                let world_pos = brush_global.transform_point(centroid);
                if let Ok(screen_pos) = camera.world_to_viewport(cam_tf, world_pos) {
                    let dist = (screen_pos - viewport_cursor).length();
                    if dist < best_dist {
                        best_dist = dist;
                        best_split = Some(centroid);
                    }
                }
            }
        }

        if let Some(split_pos) = best_split {
            let new_idx = cache.vertices.len();
            brush_selection.vertices = vec![new_idx];
            drag_state.split_vertex = Some(split_pos);
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
        }
        return;
    }

    // Normal vertex picking
    let mut best_vert = None;
    let mut best_dist = 20.0_f32;

    for (vi, v) in cache.vertices.iter().enumerate() {
        let world_pos = brush_global.transform_point(*v);
        if let Ok(screen_pos) = camera.world_to_viewport(cam_tf, world_pos) {
            let dist = (screen_pos - viewport_cursor).length();
            if dist < best_dist {
                best_dist = dist;
                best_vert = Some(vi);
            }
        }
    }

    if let Some(vi) = best_vert {
        if ctrl {
            // Ctrl+click: toggle multi-select, no drag
            if let Some(pos) = brush_selection.vertices.iter().position(|&v| v == vi) {
                brush_selection.vertices.remove(pos);
            } else {
                brush_selection.vertices.push(vi);
            }
        } else {
            brush_selection.vertices = vec![vi];
            // Record pending drag
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
        }
    } else if !ctrl {
        // Click outside any vertex: exit to Object mode
        *edit_mode = EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }
}

pub(super) fn brush_edge_interact(
    mut edit_mode: ResMut<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    input: KeyboardInput,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    brush_transforms: Query<&GlobalTransform>,
    mut brush_selection: ResMut<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    mut brushes: Query<&mut Brush>,
    mut drag_state: ResMut<EdgeDragState>,
    input_focus: Res<InputFocus>,
    mut history: ResMut<CommandHistory>,
    snap_settings: Res<crate::snapping::SnapSettings>,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;
    let EditMode::BrushEdit(BrushEditMode::Edge) = *edit_mode else {
        drag_state.active = false;
        drag_state.pending = None;
        return;
    };
    if input_focus.0.is_some() {
        return;
    }

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };

    // PageUp/PageDown: nudge selected edge vertices vertically (no cursor needed)
    if !drag_state.active && drag_state.pending.is_none() && !brush_selection.edges.is_empty() {
        let nudge_dir = if keybinds.key_just_pressed(EditorAction::NudgeUp, keyboard) {
            Some(1.0)
        } else if keybinds.key_just_pressed(EditorAction::NudgeDown, keyboard) {
            Some(-1.0)
        } else {
            None
        };
        if let Some(dir) = nudge_dir {
            let grid = snap_settings.grid_size();
            if let Ok(cache) = brush_caches.get(brush_entity)
                && let Ok(mut brush) = brushes.get_mut(brush_entity)
            {
                let old = brush.clone();
                let offset = Vec3::new(0.0, dir * grid, 0.0);
                let mut new_verts = cache.vertices.clone();
                let mut seen = HashSet::new();
                for &(a, b) in &brush_selection.edges {
                    if seen.insert(a) && a < new_verts.len() {
                        new_verts[a] += offset;
                    }
                    if seen.insert(b) && b < new_verts.len() {
                        new_verts[b] += offset;
                    }
                }
                if let Some((new_brush, _)) = rebuild_brush_from_vertices(
                    &old,
                    &cache.vertices,
                    &cache.face_polygons,
                    &new_verts,
                ) {
                    *brush = new_brush;
                    let cmd = SetBrush {
                        entity: brush_entity,
                        old,
                        new: brush.clone(),
                        label: "Nudge brush edge".to_string(),
                    };
                    history.push_executed(Box::new(cmd));
                }
            }
            return;
        }
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    // Axis constraint toggle during active drag
    if drag_state.active {
        if keybinds.just_pressed(EditorAction::ConstrainX, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisX {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisX
            };
        } else if keybinds.just_pressed(EditorAction::ConstrainY, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisY {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisY
            };
        } else if keybinds.just_pressed(EditorAction::ConstrainZ, keyboard) {
            drag_state.constraint = if drag_state.constraint == VertexDragConstraint::AxisZ {
                VertexDragConstraint::Free
            } else {
                VertexDragConstraint::AxisZ
            };
        }
    }

    // Cancel active drag on Escape or right-click
    if drag_state.active
        && (keybinds.just_pressed(EditorAction::ExitEditMode, keyboard)
            || mouse.just_pressed(MouseButton::Right))
    {
        if let Some(ref start) = drag_state.start_brush
            && let Ok(mut brush) = brushes.get_mut(brush_entity)
        {
            *brush = start.clone();
        }
        drag_state.active = false;
        drag_state.pending = None;
        drag_state.constraint = VertexDragConstraint::Free;
        return;
    }

    // Release: commit drag
    if mouse.just_released(MouseButton::Left) {
        if drag_state.active {
            if let Some(ref start) = drag_state.start_brush
                && let Ok(brush) = brushes.get(brush_entity)
            {
                let cmd = SetBrush {
                    entity: brush_entity,
                    old: start.clone(),
                    new: brush.clone(),
                    label: "Move brush edge".to_string(),
                };
                history.push_executed(Box::new(cmd));
            }
            drag_state.active = false;
            drag_state.constraint = VertexDragConstraint::Free;
        }
        drag_state.pending = None;
        return;
    }

    // Pending → active promotion (5px threshold)
    if let Some(ref pending) = drag_state.pending
        && mouse.pressed(MouseButton::Left)
        && !drag_state.active
    {
        let dist = (cursor_pos - pending.click_pos).length();
        if dist > 5.0
            && let Ok(cache) = brush_caches.get(brush_entity)
            && let Ok(brush) = brushes.get(brush_entity)
        {
            drag_state.active = true;
            drag_state.constraint = VertexDragConstraint::Free;
            drag_state.start_brush = Some(brush.clone());
            drag_state.start_cursor = viewport_cursor;
            drag_state.start_all_vertices = cache.vertices.clone();
            drag_state.start_face_polygons = cache.face_polygons.clone();

            let mut seen = HashSet::new();
            let mut edge_verts = Vec::new();
            for &(a, b) in &brush_selection.edges {
                if seen.insert(a) {
                    let pos = cache.vertices.get(a).copied().unwrap_or(Vec3::ZERO);
                    edge_verts.push((a, pos));
                }
                if seen.insert(b) {
                    let pos = cache.vertices.get(b).copied().unwrap_or(Vec3::ZERO);
                    edge_verts.push((b, pos));
                }
            }
            drag_state.start_edge_vertices = edge_verts;
        }
    }

    // Continue active drag
    if drag_state.active {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            drag_state.active = false;
            return;
        };
        let Some(ref start) = drag_state.start_brush else {
            drag_state.active = false;
            return;
        };
        let Ok(brush_global) = brush_transforms.get(brush_entity) else {
            return;
        };

        let mouse_delta = viewport_cursor - drag_state.start_cursor;
        let Some(local_offset) = compute_brush_drag_offset(
            drag_state.constraint,
            mouse_delta,
            cam_tf,
            camera,
            brush_global,
        ) else {
            return;
        };

        let mut new_verts = drag_state.start_all_vertices.clone();
        for &(vi, start_pos) in &drag_state.start_edge_vertices {
            if vi < new_verts.len() {
                new_verts[vi] = start_pos + local_offset;
            }
        }

        if let Some((new_brush, _)) = rebuild_brush_from_vertices(
            start,
            &drag_state.start_all_vertices,
            &drag_state.start_face_polygons,
            &new_verts,
        ) {
            *brush = new_brush;
        }
        return;
    }

    // Mouse press: pick edge and start pending drag
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok(cache) = brush_caches.get(brush_entity) else {
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return;
    };

    // Collect unique edges from face polygons
    let mut unique_edges: Vec<(usize, usize)> = Vec::new();
    for polygon in &cache.face_polygons {
        if polygon.len() < 2 {
            continue;
        }
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let edge = (a.min(b), a.max(b));
            if !unique_edges.contains(&edge) {
                unique_edges.push(edge);
            }
        }
    }

    let mut best_edge = None;
    let mut best_dist = 20.0_f32;

    for &(a, b) in &unique_edges {
        let wa = brush_global.transform_point(cache.vertices[a]);
        let wb = brush_global.transform_point(cache.vertices[b]);
        let Ok(sa) = camera.world_to_viewport(cam_tf, wa) else {
            continue;
        };
        let Ok(sb) = camera.world_to_viewport(cam_tf, wb) else {
            continue;
        };
        let dist = point_to_segment_dist(viewport_cursor, sa, sb);
        if dist < best_dist {
            best_dist = dist;
            best_edge = Some((a, b));
        }
    }

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if let Some(edge) = best_edge {
        if ctrl {
            // Ctrl+click: toggle multi-select, no drag
            if let Some(pos) = brush_selection.edges.iter().position(|e| *e == edge) {
                brush_selection.edges.remove(pos);
            } else {
                brush_selection.edges.push(edge);
            }
        } else {
            brush_selection.edges = vec![edge];
            // Record pending drag
            drag_state.pending = Some(PendingSubDrag {
                click_pos: cursor_pos,
            });
        }
    } else if !ctrl {
        // Click outside any edge: exit to Object mode
        *edit_mode = EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }
}

pub(crate) struct PendingSubDrag {
    pub click_pos: Vec2,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum FaceExtrudeMode {
    #[default]
    Merge, // Push/pull existing face plane
    Extend, // Create new brush from face extrusion
}

#[derive(Resource, Default)]
pub(crate) struct BrushDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub extrude_mode: FaceExtrudeMode,
    /// When true, exits to Object mode when drag completes or is cancelled.
    pub quick_action: bool,
    start_brush: Option<Brush>,
    start_cursor: Vec2,
    drag_face_normal: Vec3,
    /// World-space face polygon vertices for extend preview.
    pub extend_face_polygon: Vec<Vec3>,
    /// World-space face normal for extend preview.
    pub extend_face_normal: Vec3,
    /// Current extrude depth during extend drag.
    pub extend_depth: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum VertexDragConstraint {
    #[default]
    Free,
    AxisX,
    AxisY,
    AxisZ,
}

#[derive(Resource, Default)]
pub(crate) struct VertexDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub constraint: VertexDragConstraint,
    start_brush: Option<Brush>,
    start_cursor: Vec2,
    start_vertex_positions: Vec<Vec3>,
    /// Full vertex list at drag start (for hull rebuild).
    start_all_vertices: Vec<Vec3>,
    /// Per-face polygon indices at drag start (for hull rebuild).
    start_face_polygons: Vec<Vec<usize>>,
    /// New vertex position for Shift+drag split (edge midpoint or face center).
    split_vertex: Option<Vec3>,
}

/// Compute a local-space offset for brush vertex/edge drag based on mouse movement.
fn compute_brush_drag_offset(
    constraint: VertexDragConstraint,
    mouse_delta: Vec2,
    cam_tf: &GlobalTransform,
    camera: &Camera,
    brush_global: &GlobalTransform,
) -> Option<Vec3> {
    let brush_pos = brush_global.translation();
    let cam_dist = (cam_tf.translation() - brush_pos).length();
    let scale = cam_dist * 0.003;

    let offset = match constraint {
        VertexDragConstraint::Free => {
            let cam_right = cam_tf.right().as_vec3();
            let cam_up = cam_tf.up().as_vec3();
            let world_offset =
                cam_right * mouse_delta.x * scale + cam_up * (-mouse_delta.y) * scale;
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            brush_rot.inverse() * world_offset
        }
        constraint => {
            let axis_dir = match constraint {
                VertexDragConstraint::AxisX => Vec3::X,
                VertexDragConstraint::AxisY => Vec3::Y,
                VertexDragConstraint::AxisZ => Vec3::Z,
                VertexDragConstraint::Free => unreachable!(),
            };
            let origin_screen = camera.world_to_viewport(cam_tf, brush_pos).ok()?;
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            let world_axis = brush_rot * axis_dir;
            let axis_screen = camera
                .world_to_viewport(cam_tf, brush_pos + world_axis)
                .ok()?;
            let screen_axis = (axis_screen - origin_screen).normalize_or_zero();
            let projected = mouse_delta.dot(screen_axis);
            axis_dir * projected * scale
        }
    };
    Some(offset)
}

#[derive(Resource, Default)]
pub(crate) struct EdgeDragState {
    pub pending: Option<PendingSubDrag>,
    pub active: bool,
    pub constraint: VertexDragConstraint,
    start_brush: Option<Brush>,
    start_cursor: Vec2,
    /// Start positions for each selected edge's two endpoints (vertex indices + positions).
    start_edge_vertices: Vec<(usize, Vec3)>,
    /// Full vertex list at drag start (for hull rebuild).
    start_all_vertices: Vec<Vec3>,
    /// Per-face polygon indices at drag start (for hull rebuild).
    start_face_polygons: Vec<Vec<usize>>,
}

pub(super) fn handle_brush_delete(
    edit_mode: Res<EditMode>,
    input: KeyboardInput,
    input_focus: Res<InputFocus>,
    mut brush_selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_caches: Query<&BrushMeshCache>,
    mut history: ResMut<CommandHistory>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    face_drag: Res<BrushDragState>,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;

    let EditMode::BrushEdit(mode) = *edit_mode else {
        return;
    };
    if input_focus.0.is_some() {
        return;
    }
    if !keybinds.just_pressed(EditorAction::DeleteBrushElement, keyboard) {
        return;
    }
    // Don't delete while dragging
    if vertex_drag.active || edge_drag.active || face_drag.active {
        return;
    }

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return;
    };

    match mode {
        BrushEditMode::Vertex => {
            if brush_selection.vertices.is_empty() {
                return;
            }
            let Ok(cache) = brush_caches.get(brush_entity) else {
                return;
            };
            let remove_set: HashSet<usize> = brush_selection.vertices.iter().copied().collect();
            let remaining: Vec<Vec3> = cache
                .vertices
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove_set.contains(i))
                .map(|(_, v)| *v)
                .collect();
            if remaining.len() < 4 {
                return; // need at least a tetrahedron
            }
            let old = brush.clone();
            if let Some((new_brush, _)) =
                rebuild_brush_from_vertices(&old, &cache.vertices, &cache.face_polygons, &remaining)
            {
                *brush = new_brush;
                let cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: brush.clone(),
                    label: "Remove brush vertex".to_string(),
                };
                history.push_executed(Box::new(cmd));
                brush_selection.vertices.clear();
            }
        }
        BrushEditMode::Edge => {
            if brush_selection.edges.is_empty() {
                return;
            }
            let Ok(cache) = brush_caches.get(brush_entity) else {
                return;
            };
            let mut remove_set = HashSet::new();
            for &(a, b) in &brush_selection.edges {
                remove_set.insert(a);
                remove_set.insert(b);
            }
            let remaining: Vec<Vec3> = cache
                .vertices
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove_set.contains(i))
                .map(|(_, v)| *v)
                .collect();
            if remaining.len() < 4 {
                return;
            }
            let old = brush.clone();
            if let Some((new_brush, _)) =
                rebuild_brush_from_vertices(&old, &cache.vertices, &cache.face_polygons, &remaining)
            {
                *brush = new_brush;
                let cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: brush.clone(),
                    label: "Remove brush edge".to_string(),
                };
                history.push_executed(Box::new(cmd));
                brush_selection.edges.clear();
            }
        }
        BrushEditMode::Face => {
            if brush_selection.faces.is_empty() {
                return;
            }
            let remaining = brush.faces.len() - brush_selection.faces.len();
            if remaining < 4 {
                return;
            }
            let old = brush.clone();
            let remove_set: HashSet<usize> = brush_selection.faces.iter().copied().collect();
            let new_faces: Vec<BrushFaceData> = brush
                .faces
                .iter()
                .enumerate()
                .filter(|(i, _)| !remove_set.contains(i))
                .map(|(_, f)| f.clone())
                .collect();
            brush.faces = new_faces;
            let cmd = SetBrush {
                entity: brush_entity,
                old,
                new: brush.clone(),
                label: "Remove brush face".to_string(),
            };
            history.push_executed(Box::new(cmd));
            brush_selection.faces.clear();
        }
        _ => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ClipMode {
    #[default]
    KeepFront,
    KeepBack,
    Split,
}

#[derive(Resource, Default)]
pub(crate) struct ClipState {
    pub points: Vec<Vec3>,
    pub preview_plane: Option<BrushPlane>,
    pub mode: ClipMode,
}

pub(super) fn handle_clip_mode(
    edit_mode: Res<EditMode>,
    input: KeyboardInput,
    mouse: Res<ButtonInput<MouseButton>>,
    input_focus: Res<InputFocus>,
    windows: Query<&Window>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    brush_selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    brush_transforms: Query<&GlobalTransform>,
    brush_caches: Query<&BrushMeshCache>,
    mut clip_state: ResMut<ClipState>,
    mut history: ResMut<CommandHistory>,
    snap_settings: Res<crate::snapping::SnapSettings>,
    mut commands: Commands,
    mut gizmos: Gizmos,
) {
    let keyboard = &input.keyboard;
    let keybinds = &input.keybinds;
    let EditMode::BrushEdit(BrushEditMode::Clip) = *edit_mode else {
        // Clear clip state when not in clip mode
        if !clip_state.points.is_empty() || clip_state.mode != ClipMode::KeepFront {
            clip_state.points.clear();
            clip_state.preview_plane = None;
            clip_state.mode = ClipMode::KeepFront;
        }
        return;
    };
    if input_focus.0.is_some() {
        return;
    }

    let Some(brush_entity) = brush_selection.entity else {
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };

    // Escape clears clip points and resets mode
    if keybinds.just_pressed(EditorAction::ClipClear, keyboard) {
        clip_state.points.clear();
        clip_state.preview_plane = None;
        clip_state.mode = ClipMode::KeepFront;
        return;
    }

    // Tab cycles clip mode when preview plane exists
    if keybinds.just_pressed(EditorAction::ClipCycleMode, keyboard)
        && clip_state.preview_plane.is_some()
    {
        clip_state.mode = match clip_state.mode {
            ClipMode::KeepFront => ClipMode::KeepBack,
            ClipMode::KeepBack => ClipMode::Split,
            ClipMode::Split => ClipMode::KeepFront,
        };
    }

    // Left click: add point by raycasting to brush surface
    if mouse.just_pressed(MouseButton::Left) && clip_state.points.len() < 3 {
        let Some(cursor_pos) = window.cursor_position() else {
            return;
        };
        let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
        else {
            return;
        };

        // Cast ray from camera through cursor
        let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
            return;
        };

        let Ok(cache) = brush_caches.get(brush_entity) else {
            return;
        };

        // Find closest intersection with any brush face
        let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
        let mut best_t = f32::MAX;
        let mut best_point = None;

        for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
            if polygon.len() < 3 {
                continue;
            }
            let Ok(brush_ref) = brushes.get(brush_entity) else {
                return;
            };
            let face = &brush_ref.faces[face_idx];
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
                // Verify hit is roughly on the brush (within face polygon bounds)
                let local_hit = brush_rot.inverse() * (hit - brush_trans);
                if point_inside_all_planes(local_hit, &brush_ref.faces) {
                    best_t = t;
                    best_point = Some(local_hit);
                }
            }
        }

        if let Some(mut point) = best_point {
            // Grid snap the clip point
            let world_point = brush_global.transform_point(point);
            let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
            let snapped = snap_settings.snap_translate_vec3_if(world_point, ctrl);
            let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
            point = brush_rot.inverse() * (snapped - brush_trans);
            clip_state.points.push(point);
        }
    }

    // Compute preview plane from collected points
    clip_state.preview_plane = match clip_state.points.len() {
        2 => {
            // Two points + camera forward for orientation
            let dir = clip_state.points[1] - clip_state.points[0];
            let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
            let local_cam_fwd = brush_rot.inverse() * cam_tf.forward().as_vec3();
            let normal = dir.cross(local_cam_fwd).normalize_or_zero();
            if normal.length_squared() > 0.5 {
                let distance = normal.dot(clip_state.points[0]);
                Some(BrushPlane { normal, distance })
            } else {
                None
            }
        }
        3 => {
            let a = clip_state.points[0];
            let b = clip_state.points[1];
            let c = clip_state.points[2];
            let normal = (b - a).cross(c - a).normalize_or_zero();
            if normal.length_squared() > 0.5 {
                let distance = normal.dot(a);
                Some(BrushPlane { normal, distance })
            } else {
                None
            }
        }
        _ => None,
    };

    // Enter: apply clip plane based on mode
    if keybinds.just_pressed(EditorAction::ClipApply, keyboard)
        && let Some(ref plane) = clip_state.preview_plane
    {
        let Ok(mut brush) = brushes.get_mut(brush_entity) else {
            return;
        };
        let (clip_u, clip_v) = compute_face_tangent_axes(plane.normal);
        let clip_face = BrushFaceData {
            plane: plane.clone(),
            uv_offset: Vec2::ZERO,
            uv_scale: Vec2::ONE,
            uv_rotation: 0.0,
            uv_u_axis: clip_u,
            uv_v_axis: clip_v,
            ..default()
        };
        let (flip_u, flip_v) = compute_face_tangent_axes(-plane.normal);
        let flipped_face = BrushFaceData {
            plane: BrushPlane {
                normal: -plane.normal,
                distance: -plane.distance,
            },
            uv_u_axis: flip_u,
            uv_v_axis: flip_v,
            ..clip_face.clone()
        };

        match clip_state.mode {
            ClipMode::KeepFront => {
                let old = brush.clone();
                brush.faces.push(clip_face);
                let cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: brush.clone(),
                    label: "Clip brush (keep front)".to_string(),
                };
                history.push_executed(Box::new(cmd));
            }
            ClipMode::KeepBack => {
                let old = brush.clone();
                brush.faces.push(flipped_face);
                let cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: brush.clone(),
                    label: "Clip brush (keep back)".to_string(),
                };
                history.push_executed(Box::new(cmd));
            }
            ClipMode::Split => {
                let old = brush.clone();
                // Front half: apply clip plane to original
                let mut front = old.clone();
                front.faces.push(clip_face);
                // Back half: apply flipped clip plane
                let mut back = old.clone();
                back.faces.push(flipped_face);

                // Update original entity to front half
                *brush = front.clone();
                let set_cmd = SetBrush {
                    entity: brush_entity,
                    old,
                    new: front,
                    label: "Clip brush (split - front)".to_string(),
                };

                // Spawn back half as new entity
                let back_brush = back;
                let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();
                let spawn_transform = Transform {
                    translation: brush_trans,
                    rotation: brush_rot,
                    scale: Vec3::ONE,
                };
                let back_owned = back_brush.clone();
                let transform_owned = spawn_transform;
                commands.queue(move |world: &mut World| {
                    let parent_group = world
                        .get::<ChildOf>(brush_entity)
                        .map(|c| c.0)
                        .filter(|&p| world.get::<BrushGroup>(p).is_some());

                    let actual_transform = if parent_group.is_some() {
                        *world.get::<Transform>(brush_entity).unwrap()
                    } else {
                        transform_owned
                    };

                    let mut spawner = world.spawn((
                        Name::new("Brush"),
                        back_owned,
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

                    let group = crate::commands::CommandGroup {
                        commands: vec![Box::new(set_cmd), Box::new(create_cmd)],
                        label: "Split brush".to_string(),
                    };
                    let mut history = world.resource_mut::<CommandHistory>();
                    history.push_executed(Box::new(group));
                });
                clip_state.points.clear();
                clip_state.preview_plane = None;
                clip_state.mode = ClipMode::KeepFront;
                return;
            }
        }

        clip_state.points.clear();
        clip_state.preview_plane = None;
        clip_state.mode = ClipMode::KeepFront;
    }

    // Draw clip points and preview
    for (i, point) in clip_state.points.iter().enumerate() {
        let world_pos = brush_global.transform_point(*point);
        let color = default_style::CLIP_POINT;
        gizmos.sphere(Isometry3d::from_translation(world_pos), 0.06, color);
        // Draw connecting lines between points
        if i > 0 {
            let prev_world = brush_global.transform_point(clip_state.points[i - 1]);
            gizmos.line(prev_world, world_pos, color);
        }
    }

    // Draw clipped geometry preview
    if let Some(ref plane) = clip_state.preview_plane {
        let Ok(brush_ref) = brushes.get(brush_entity) else {
            return;
        };
        let (_, brush_rot, brush_trans) = brush_global.to_scale_rotation_translation();

        let world_faces = brush_planes_to_world(&brush_ref.faces, brush_rot, brush_trans);

        // Transform clip plane to world space (same formula as brush_planes_to_world)
        let world_clip_normal = (brush_rot * plane.normal).normalize();
        let world_clip_distance = plane.distance + world_clip_normal.dot(brush_trans);

        // Front half faces (brush + clip plane)
        let front_clip = BrushFaceData {
            plane: BrushPlane {
                normal: world_clip_normal,
                distance: world_clip_distance,
            },
            uv_scale: Vec2::ONE,
            ..default()
        };
        let mut front_faces = world_faces.clone();
        front_faces.push(front_clip);

        // Back half faces (brush + flipped clip plane)
        let back_clip = BrushFaceData {
            plane: BrushPlane {
                normal: -world_clip_normal,
                distance: -world_clip_distance,
            },
            uv_scale: Vec2::ONE,
            ..default()
        };
        let mut back_faces = world_faces;
        back_faces.push(back_clip);

        let (front_color, back_color) = match clip_state.mode {
            ClipMode::KeepFront => (default_style::CLIP_KEEP, default_style::CLIP_DISCARD),
            ClipMode::KeepBack => (default_style::CLIP_DISCARD, default_style::CLIP_KEEP),
            ClipMode::Split => (default_style::CLIP_KEEP, default_style::CLIP_SPLIT_BACK),
        };

        // Draw front half wireframe
        let (verts, polys) = compute_brush_geometry(&front_faces);
        if verts.len() >= 4 {
            for polygon in &polys {
                for i in 0..polygon.len() {
                    let a = verts[polygon[i]];
                    let b = verts[polygon[(i + 1) % polygon.len()]];
                    gizmos.line(a, b, front_color);
                }
            }
        }

        // Draw back half wireframe
        let (verts, polys) = compute_brush_geometry(&back_faces);
        if verts.len() >= 4 {
            for polygon in &polys {
                for i in 0..polygon.len() {
                    let a = verts[polygon[i]];
                    let b = verts[polygon[(i + 1) % polygon.len()]];
                    gizmos.line(a, b, back_color);
                }
            }
        }
    }
}

/// Pick the closest face under the cursor on a given brush entity.
fn pick_face_under_cursor(
    viewport_cursor: Vec2,
    brush_entity: Entity,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cache: &BrushMeshCache,
    face_entities: &Query<(Entity, &super::BrushFaceEntity, &GlobalTransform)>,
) -> Option<usize> {
    let mut best_face = None;
    let mut best_depth = f32::MAX;

    for (_, face_ent, face_global) in face_entities {
        if face_ent.brush_entity != brush_entity {
            continue;
        }
        let face_idx = face_ent.face_index;
        let polygon = &cache.face_polygons[face_idx];
        if polygon.len() < 3 {
            continue;
        }
        let screen_verts: Vec<Vec2> = polygon
            .iter()
            .filter_map(|&vi| {
                let world = face_global.transform_point(cache.vertices[vi]);
                camera.world_to_viewport(cam_tf, world).ok()
            })
            .collect();
        if screen_verts.len() < 3 {
            continue;
        }
        if point_in_polygon_2d(viewport_cursor, &screen_verts) {
            let centroid: Vec3 =
                polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
            let world_centroid = face_global.transform_point(centroid);
            let depth = (cam_tf.translation() - world_centroid).length_squared();
            if depth < best_depth {
                best_depth = depth;
                best_face = Some(face_idx);
            }
        }
    }
    best_face
}

/// Updates the hover resource each frame to track which face the cursor is over.
pub(super) fn brush_face_hover(
    edit_mode: Res<EditMode>,
    input: KeyboardInput,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    face_entities: Query<(Entity, &super::BrushFaceEntity, &GlobalTransform)>,
    brush_selection: Res<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    selection: Res<Selection>,
    drag_state: Res<BrushDragState>,
    mut hover: ResMut<super::BrushFaceHover>,
    brushes: Query<(), With<Brush>>,
) {
    let keyboard = &input.keyboard;
    let in_face_edit = matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Face));
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // Clear hover during active drag
    if drag_state.active {
        hover.entity = None;
        hover.face_index = None;
        return;
    }

    // Determine if we should show hover
    let should_hover = in_face_edit || (*edit_mode == EditMode::Object && (shift || alt));

    if !should_hover {
        hover.entity = None;
        hover.face_index = None;
        return;
    }

    let intent = if alt {
        super::HoverIntent::Extend
    } else {
        super::HoverIntent::PushPull
    };

    let Ok(window) = windows.single() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    let brush_entity = if in_face_edit {
        brush_selection.entity
    } else {
        selection.primary().filter(|&e| brushes.contains(e))
    };

    let Some(brush_entity) = brush_entity else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    let Ok(cache) = brush_caches.get(brush_entity) else {
        hover.entity = None;
        hover.face_index = None;
        return;
    };

    if let Some(face_idx) = pick_face_under_cursor(
        viewport_cursor,
        brush_entity,
        camera,
        cam_tf,
        cache,
        &face_entities,
    ) {
        hover.entity = Some(brush_entity);
        hover.face_index = Some(face_idx);
        hover.intent = intent;
    } else {
        hover.entity = None;
        hover.face_index = None;
    }
}
