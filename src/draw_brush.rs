use bevy::{
    input_focus::InputFocus,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    ui::UiGlobalTransform,
};

use crate::{
    EditorEntity,
    brush::BrushFaceEntity,
    commands::{
        CommandGroup, CommandHistory, DespawnEntity, EditorCommand, snapshot_entity,
        snapshot_rebuild,
    },
    selection::{Selected, Selection},
    snapping::SnapSettings,
    viewport::SceneViewport,
    viewport_util::window_to_viewport_cursor,
};
use jackdaw_geometry::{
    brush_planes_to_world, brushes_intersect, clean_degenerate_faces, compute_brush_geometry,
    compute_face_tangent_axes, subtract_brush,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushPlane};

const EXTRUDE_DEPTH_SENSITIVITY: f32 = 0.003;
const MIN_FOOTPRINT_SIZE: f32 = 0.01;
const MIN_EXTRUDE_DEPTH: f32 = 0.01;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawPhase {
    PlacingFirstCorner,
    DrawingFootprint,
    DrawingPolygon,
    ExtrudingDepth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DrawMode {
    #[default]
    Add,
    Cut,
}

#[derive(Clone, Debug)]
pub struct DrawPlane {
    pub origin: Vec3,
    pub normal: Vec3,
    pub axis_u: Vec3,
    pub axis_v: Vec3,
}

#[derive(Clone, Debug)]
pub struct ActiveDraw {
    pub corner1: Vec3,
    pub corner2: Vec3,
    pub depth: f32,
    pub phase: DrawPhase,
    pub mode: DrawMode,
    pub plane: DrawPlane,
    pub extrude_start_cursor: Vec2,
    pub plane_locked: bool,
    /// World-space cursor position on the drawing plane (for crosshair preview).
    pub cursor_on_plane: Option<Vec3>,
    /// When set, the drawn shape will be CSG-unioned with this brush instead of spawning a new entity.
    pub append_target: Option<Entity>,
    /// True during press-drag-release rectangle drawing.
    pub drag_footprint: bool,
    /// Screen position at initial press (for drag vs click detection).
    pub press_screen_pos: Option<Vec2>,
    /// Placed polygon vertices in world space (polygon draw mode).
    pub polygon_vertices: Vec<Vec3>,
    /// Current cursor position on plane during polygon mode (for preview edge).
    pub polygon_cursor: Option<Vec3>,
}

#[derive(Resource, Default)]
pub struct DrawBrushState {
    pub active: Option<ActiveDraw>,
}

pub struct CreateBrushCommand {
    pub entity: Entity,
    pub scene_snapshot: DynamicScene,
}

impl EditorCommand for CreateBrushCommand {
    fn execute(&self, world: &mut World) {
        // Redo: respawn from snapshot
        let scene = snapshot_rebuild(&self.scene_snapshot);
        let _result = scene.write_to_world(world, &mut Default::default());
    }

    fn undo(&self, world: &mut World) {
        if let Ok(entity_mut) = world.get_entity_mut(self.entity) {
            entity_mut.despawn();
        }
    }

    fn description(&self) -> &str {
        "Draw brush"
    }
}

pub struct DrawBrushPlugin;

impl Plugin for DrawBrushPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DrawBrushState>().add_systems(
            Update,
            (
                draw_brush_activate,
                draw_brush_update,
                draw_brush_release,
                draw_brush_confirm,
                draw_brush_cancel,
                draw_brush_preview,
                join_selected_brushes,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
    }
}

fn draw_brush_activate(
    keyboard: Res<ButtonInput<KeyCode>>,
    input_focus: Res<InputFocus>,
    mut draw_state: ResMut<DrawBrushState>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    mut edit_mode: ResMut<crate::brush::EditMode>,
    mut brush_selection: ResMut<crate::brush::BrushSelection>,
    selection: Res<Selection>,
    brush_query: Query<(), With<Brush>>,
) {
    // Handle Tab toggle while in draw mode (works in all phases)
    if let Some(ref mut active) = draw_state.active {
        if keyboard.just_pressed(KeyCode::Tab) {
            active.mode = match active.mode {
                DrawMode::Add => DrawMode::Cut,
                DrawMode::Cut => DrawMode::Add,
            };
        }
        return;
    }

    // B = draw in Add mode, C = draw in Cut mode
    let mode = if keyboard.just_pressed(KeyCode::KeyB) {
        DrawMode::Add
    } else if keyboard.just_pressed(KeyCode::KeyC) {
        DrawMode::Cut
    } else {
        return;
    };
    // Standard guards
    if input_focus.0.is_some() || modal.active.is_some() {
        return;
    }

    // Only append to selected brush when Alt is held; otherwise always create new
    let append_target = if mode == DrawMode::Add {
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        if alt {
            selection.primary().filter(|&e| brush_query.contains(e))
        } else {
            None
        }
    } else {
        None
    };

    // Exit brush edit mode if active
    if *edit_mode != crate::brush::EditMode::Object {
        *edit_mode = crate::brush::EditMode::Object;
        brush_selection.entity = None;
        brush_selection.faces.clear();
        brush_selection.vertices.clear();
        brush_selection.edges.clear();
    }

    draw_state.active = Some(ActiveDraw {
        corner1: Vec3::ZERO,
        corner2: Vec3::ZERO,
        depth: 0.0,
        phase: DrawPhase::PlacingFirstCorner,
        mode,
        plane: DrawPlane {
            origin: Vec3::ZERO,
            normal: Vec3::Y,
            axis_u: Vec3::X,
            axis_v: Vec3::Z,
        },
        extrude_start_cursor: Vec2::ZERO,
        plane_locked: false,
        cursor_on_plane: None,
        append_target,
        drag_footprint: false,
        press_screen_pos: None,
        polygon_vertices: Vec::new(),
        polygon_cursor: None,
    });
}

fn draw_brush_update(
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<EditorEntity>)>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    snap_settings: Res<SnapSettings>,
    mut ray_cast: MeshRayCast,
    brush_faces: Query<(&BrushFaceEntity, &GlobalTransform)>,
    brushes: Query<(&Brush, &GlobalTransform)>,
) {
    let Some(ref mut active) = draw_state.active else {
        return;
    };

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
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return;
    };

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            // Ctrl toggles plane lock
            active.plane_locked = ctrl;

            if !active.plane_locked {
                // Raycast against brush face meshes
                let settings =
                    MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
                let hits = ray_cast.cast_ray(ray, &settings);

                let mut found_face = false;
                for (hit_entity, hit_data) in hits {
                    if let Ok((face_ent, _face_tf)) = brush_faces.get(*hit_entity) {
                        if let Ok((brush, brush_tf)) = brushes.get(face_ent.brush_entity) {
                            let face = &brush.faces[face_ent.face_index];
                            let (_, brush_rot, _) = brush_tf.to_scale_rotation_translation();
                            let world_normal = (brush_rot * face.plane.normal).normalize();
                            let hit_point = hit_data.point;

                            let (u, v) = compute_face_tangent_axes(world_normal);
                            active.plane = DrawPlane {
                                origin: hit_point,
                                normal: world_normal,
                                axis_u: u,
                                axis_v: v,
                            };
                            found_face = true;
                            break;
                        }
                    }
                }

                if !found_face {
                    // Fall back to Y=0 ground plane
                    if let Some(ground_hit) = ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y) {
                        active.plane = DrawPlane {
                            origin: ground_hit,
                            normal: Vec3::Y,
                            axis_u: Vec3::X,
                            axis_v: Vec3::Z,
                        };
                    }
                }
            }

            // Project cursor onto current plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, ctrl);
                active.cursor_on_plane = Some(snapped);
            }
        }
        DrawPhase::DrawingFootprint => {
            // Project cursor onto the locked drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, ctrl);
                active.corner2 = snapped;
            }
        }
        DrawPhase::DrawingPolygon => {
            // Project cursor onto drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, ctrl);
                active.polygon_cursor = Some(snapped);
            }
        }
        DrawPhase::ExtrudingDepth => {
            // Use polygon centroid if in polygon mode, otherwise rectangle midpoint
            let center = if !active.polygon_vertices.is_empty() {
                active.polygon_vertices.iter().sum::<Vec3>() / active.polygon_vertices.len() as f32
            } else {
                (active.corner1 + active.corner2) / 2.0
            };
            let cam_dist = (cam_tf.translation() - center).length();

            // Project the plane normal to screen space to determine drag direction
            if let (Ok(origin_screen), Ok(normal_screen)) = (
                camera.world_to_viewport(cam_tf, center),
                camera.world_to_viewport(cam_tf, center + active.plane.normal),
            ) {
                let screen_dir = (normal_screen - origin_screen).normalize_or_zero();
                let mouse_delta = viewport_cursor - active.extrude_start_cursor;
                let projected = mouse_delta.dot(screen_dir);
                let raw_depth = projected * cam_dist * EXTRUDE_DEPTH_SENSITIVITY;

                // Snap depth
                let depth = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (raw_depth / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    raw_depth
                };
                active.depth = depth;
            }
        }
    }
}

fn draw_brush_release(
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<EditorEntity>)>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) {
    if !mouse.just_released(MouseButton::Left) {
        return;
    }

    let Some(ref mut active) = draw_state.active else {
        return;
    };

    if active.phase != DrawPhase::DrawingFootprint || !active.drag_footprint {
        return;
    }

    let Some(press_pos) = active.press_screen_pos else {
        return;
    };

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, _)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    let screen_dist = (cursor_pos - press_pos).length();
    if screen_dist > 5.0 {
        // Real drag: check footprint size, transition to ExtrudingDepth
        let delta = active.corner2 - active.corner1;
        let u_size = delta.dot(active.plane.axis_u).abs();
        let v_size = delta.dot(active.plane.axis_v).abs();
        if u_size >= MIN_FOOTPRINT_SIZE && v_size >= MIN_FOOTPRINT_SIZE {
            active.phase = DrawPhase::ExtrudingDepth;
            active.extrude_start_cursor = viewport_cursor;
            active.depth = 0.0;
        }
    } else {
        // Click (no drag): enter polygon mode
        active.phase = DrawPhase::DrawingPolygon;
        active.polygon_vertices = vec![active.corner1];
        active.drag_footprint = false;
    }
    active.press_screen_pos = None;
}

fn draw_brush_confirm(
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<EditorEntity>)>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }

    let Some(ref mut active) = draw_state.active else {
        return;
    };

    // Verify cursor is in viewport
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, _)) = camera_query.single() else {
        return;
    };
    let Some(viewport_cursor) = window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            if let Some(pos) = active.cursor_on_plane {
                active.corner1 = pos;
                active.corner2 = pos;
                active.phase = DrawPhase::DrawingFootprint;
                active.drag_footprint = true;
                active.press_screen_pos = Some(cursor_pos);
            }
        }
        DrawPhase::DrawingFootprint => {
            // Only handle non-drag mode (legacy click-move-click path, not used in normal flow)
            if active.drag_footprint {
                return;
            }
            // Enforce minimum size
            let delta = active.corner2 - active.corner1;
            let u_size = delta.dot(active.plane.axis_u).abs();
            let v_size = delta.dot(active.plane.axis_v).abs();
            if u_size < MIN_FOOTPRINT_SIZE || v_size < MIN_FOOTPRINT_SIZE {
                return; // Too small, keep drawing
            }
            active.phase = DrawPhase::ExtrudingDepth;
            active.extrude_start_cursor = viewport_cursor;
            active.depth = 0.0;
        }
        DrawPhase::DrawingPolygon => {
            if let Some(cursor) = active.polygon_cursor {
                // Accept all vertices, but skip near-duplicates
                let too_close = active
                    .polygon_vertices
                    .iter()
                    .any(|&v| (v - cursor).length() < 0.05);
                if !too_close {
                    active.polygon_vertices.push(cursor);
                }
            }
        }
        DrawPhase::ExtrudingDepth => {
            if active.depth.abs() < MIN_EXTRUDE_DEPTH {
                return; // No depth, keep extruding
            }
            let active_owned = active.clone();
            if !active_owned.polygon_vertices.is_empty() {
                // Polygon mode
                draw_state.active = None;
                if active_owned.append_target.is_some() {
                    append_to_brush(&active_owned, &mut commands);
                } else {
                    spawn_polygon_brush(&active_owned, &mut commands);
                }
            } else {
                match active_owned.mode {
                    DrawMode::Add => {
                        draw_state.active = None;
                        if active_owned.append_target.is_some() {
                            append_to_brush(&active_owned, &mut commands);
                        } else {
                            spawn_drawn_brush(&active_owned, &mut commands);
                        }
                    }
                    DrawMode::Cut => {
                        subtract_drawn_brush(&active_owned, &mut commands);
                        commands.queue(|world: &mut World| {
                            world.resource_mut::<DrawBrushState>().active = None;
                        });
                    }
                }
            }
        }
    }
}

fn draw_brush_cancel(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<EditorEntity>)>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) {
    let Some(ref mut active) = draw_state.active else {
        return;
    };

    // Polygon mode: Enter closes polygon (via convex hull), Backspace removes last vertex
    if active.phase == DrawPhase::DrawingPolygon {
        if keyboard.just_pressed(KeyCode::Enter) {
            let hull = convex_hull_on_plane(&active.polygon_vertices, &active.plane);
            if hull.len() >= 3 {
                active.polygon_vertices = hull;
                let viewport_cursor = (|| {
                    let window = windows.single().ok()?;
                    let cursor_pos = window.cursor_position()?;
                    let (camera, _) = camera_query.single().ok()?;
                    window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
                })();
                active.phase = DrawPhase::ExtrudingDepth;
                active.extrude_start_cursor = viewport_cursor.unwrap_or(Vec2::ZERO);
                active.depth = 0.0;
                return;
            }
        }
        if keyboard.just_pressed(KeyCode::Backspace) {
            active.polygon_vertices.pop();
            if active.polygon_vertices.is_empty() {
                active.phase = DrawPhase::PlacingFirstCorner;
            }
            return;
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        draw_state.active = None;
    }
}

const DRAW_COLOR: Color = Color::srgb(1.0, 0.6, 0.0);
const CUT_COLOR: Color = Color::srgb(1.0, 0.2, 0.2);

fn draw_brush_preview(
    draw_state: Res<DrawBrushState>,
    snap_settings: Res<SnapSettings>,
    mut gizmos: Gizmos,
    brushes: Query<(&Brush, &GlobalTransform)>,
) {
    let Some(ref active) = draw_state.active else {
        return;
    };

    let color = match active.mode {
        DrawMode::Add => DRAW_COLOR,
        DrawMode::Cut => CUT_COLOR,
    };

    // Highlight the append target brush so the user knows they're in hull mode
    if let Some(target) = active.append_target {
        if let Ok((brush, brush_tf)) = brushes.get(target) {
            let (verts, polys) = compute_brush_geometry(&brush.faces);
            for polygon in &polys {
                for i in 0..polygon.len() {
                    let a = brush_tf.transform_point(verts[polygon[i]]);
                    let b = brush_tf.transform_point(verts[polygon[(i + 1) % polygon.len()]]);
                    gizmos.line(a, b, DRAW_COLOR);
                }
            }
        }
    }

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            // Crosshair at cursor on surface
            if let Some(pos) = active.cursor_on_plane {
                let size = 0.3;
                gizmos.line(
                    pos - active.plane.axis_u * size,
                    pos + active.plane.axis_u * size,
                    color,
                );
                gizmos.line(
                    pos - active.plane.axis_v * size,
                    pos + active.plane.axis_v * size,
                    color,
                );

                // Draw plane grid overlay
                draw_plane_grid(&mut gizmos, &active.plane, pos, &snap_settings);
            }
        }
        DrawPhase::DrawingFootprint => {
            // Rectangle on the plane from corner1 to corner2
            let corners = footprint_corners(active);
            for i in 0..4 {
                gizmos.line(corners[i], corners[(i + 1) % 4], color);
            }

            // Draw plane grid overlay centered on midpoint of footprint
            let mid = (active.corner1 + active.corner2) / 2.0;
            draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
        }
        DrawPhase::DrawingPolygon => {
            let verts = &active.polygon_vertices;
            let cursor = active.polygon_cursor;

            // Draw all placed vertices as small spheres
            for &v in verts.iter() {
                gizmos.sphere(Isometry3d::from_translation(v), 0.04, color);
            }

            // Compute and draw the convex hull outline
            let hull = convex_hull_on_plane(verts, &active.plane);
            if hull.len() >= 2 {
                for i in 0..hull.len() {
                    gizmos.line(hull[i], hull[(i + 1) % hull.len()], color);
                }
            }

            // Draw preview edge from last placed vertex to cursor
            if let (Some(&last), Some(cursor_pos)) = (verts.last(), cursor) {
                gizmos.line(last, cursor_pos, color);

                // Crosshair at cursor
                let size = 0.15;
                gizmos.line(
                    cursor_pos - active.plane.axis_u * size,
                    cursor_pos + active.plane.axis_u * size,
                    color,
                );
                gizmos.line(
                    cursor_pos - active.plane.axis_v * size,
                    cursor_pos + active.plane.axis_v * size,
                    color,
                );

                // Draw plane grid centered on cursor
                draw_plane_grid(&mut gizmos, &active.plane, cursor_pos, &snap_settings);
            }
        }
        DrawPhase::ExtrudingDepth => {
            let offset = active.plane.normal * active.depth;

            if !active.polygon_vertices.is_empty() {
                // Polygon prism wireframe
                let verts = &active.polygon_vertices;
                let n = verts.len();
                // Base polygon
                for i in 0..n {
                    gizmos.line(verts[i], verts[(i + 1) % n], color);
                }
                // Top polygon
                for i in 0..n {
                    gizmos.line(verts[i] + offset, verts[(i + 1) % n] + offset, color);
                }
                // Connecting edges
                for &v in verts {
                    gizmos.line(v, v + offset, color);
                }
            } else {
                // Cuboid wireframe
                let base = footprint_corners(active);
                let top: [Vec3; 4] = [
                    base[0] + offset,
                    base[1] + offset,
                    base[2] + offset,
                    base[3] + offset,
                ];
                for i in 0..4 {
                    gizmos.line(base[i], base[(i + 1) % 4], color);
                }
                for i in 0..4 {
                    gizmos.line(top[i], top[(i + 1) % 4], color);
                }
                for i in 0..4 {
                    gizmos.line(base[i], top[i], color);
                }

                // Cut mode: show intersection outlines
                if active.mode == DrawMode::Cut {
                    let cutter_planes = build_cutter_planes(active);
                    for (brush, brush_tf) in &brushes {
                        let (_, rotation, translation) = brush_tf.to_scale_rotation_translation();
                        let world_target =
                            brush_planes_to_world(&brush.faces, rotation, translation);
                        let mut combined = world_target;
                        combined.extend_from_slice(&cutter_planes);
                        let (verts, polys) = compute_brush_geometry(&combined);
                        if verts.len() < 4 {
                            continue;
                        }
                        for polygon in &polys {
                            if polygon.len() < 2 {
                                continue;
                            }
                            for i in 0..polygon.len() {
                                let a = verts[polygon[i]];
                                let b = verts[polygon[(i + 1) % polygon.len()]];
                                gizmos.line(a, b, color);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn spawn_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let plane = &active.plane;

    // Decompose corners into plane-local u/v coordinates
    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    let half_u = (max_u - min_u) / 2.0;
    let half_v = (max_v - min_v) / 2.0;
    let half_depth = active.depth.abs() / 2.0;

    // Center on the plane
    let center_on_plane =
        plane.origin + plane.axis_u * (min_u + max_u) / 2.0 + plane.axis_v * (min_v + max_v) / 2.0;
    let center = center_on_plane + plane.normal * active.depth / 2.0;

    // For ground-plane (normal=Y): axis_u=X, axis_v=Z, normal=Y
    // Brush::cuboid uses half_x, half_y, half_z in local space
    // We need to map: local X -> axis_u, local Y -> normal, local Z -> axis_v
    let brush = Brush::cuboid(half_u, half_depth, half_v);

    // Build rotation that maps local (X,Y,Z) -> (axis_u, normal, axis_v)
    let rotation = if plane.normal == Vec3::Y {
        Quat::IDENTITY
    } else if plane.normal == Vec3::NEG_Y {
        Quat::from_rotation_x(std::f32::consts::PI)
    } else {
        let target_mat = Mat3::from_cols(plane.axis_u, plane.normal, -plane.axis_v);
        Quat::from_mat3(&target_mat)
    };

    commands.queue(move |world: &mut World| {
        // Apply last-used texture to all faces
        let last_tex = world
            .resource::<crate::brush::LastUsedTexture>()
            .texture_path
            .clone();
        let mut brush = brush;
        if let Some(ref path) = last_tex {
            for face in &mut brush.faces {
                face.texture_path = Some(path.clone());
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
            // Deselect current selection
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

        // Snapshot for undo
        let snapshot = snapshot_entity(world, entity);
        let cmd = CreateBrushCommand {
            entity,
            scene_snapshot: snapshot,
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(Box::new(cmd));
        history.redo_stack.clear();
    });
}

fn append_to_brush(active: &ActiveDraw, commands: &mut Commands) {
    let Some(target_entity) = active.append_target else {
        return;
    };

    // Build the drawn shape's world-space vertices (prism from polygon or cuboid from footprint)
    let offset = active.plane.normal * active.depth;
    let drawn_verts: Vec<Vec3> = if !active.polygon_vertices.is_empty() {
        let mut verts = Vec::with_capacity(active.polygon_vertices.len() * 2);
        for &v in &active.polygon_vertices {
            verts.push(v);
            verts.push(v + offset);
        }
        verts
    } else {
        let base = footprint_corners(active);
        let mut verts = Vec::with_capacity(8);
        for corner in &base {
            verts.push(*corner);
            verts.push(*corner + offset);
        }
        verts
    };

    commands.queue(move |world: &mut World| {
        use avian3d::parry::math::Point as ParryPoint;
        use avian3d::parry::transformation::convex_hull;

        let Some(brush) = world.get::<Brush>(target_entity) else {
            return;
        };
        let old_brush = brush.clone();

        let Some(global_tf) = world.get::<GlobalTransform>(target_entity) else {
            return;
        };
        let (_, rotation, translation) = global_tf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        // Get existing brush vertices in local space, then convert drawn verts to local space
        let existing_verts = compute_brush_geometry(&old_brush.faces).0;
        let existing_count = existing_verts.len();

        let mut all_local_verts: Vec<Vec3> = existing_verts;
        for v in &drawn_verts {
            all_local_verts.push(inv_rotation * (*v - translation));
        }

        if all_local_verts.len() < 4 {
            return;
        }

        // Compute convex hull
        let points: Vec<ParryPoint<f32>> = all_local_verts
            .iter()
            .map(|v| ParryPoint::new(v.x, v.y, v.z))
            .collect();
        let (hull_verts, hull_tris) = convex_hull(&points);
        if hull_verts.len() < 4 || hull_tris.is_empty() {
            return;
        }

        let hull_positions: Vec<Vec3> = hull_verts
            .iter()
            .map(|p| Vec3::new(p.x, p.y, p.z))
            .collect();
        let hull_faces = crate::brush::merge_hull_triangles(&hull_positions, &hull_tris);
        if hull_faces.len() < 4 {
            return;
        }

        // Build new face data, matching old faces where possible for texture preservation
        let old_face_polygons = compute_brush_geometry(&old_brush.faces).1;
        let last_tex = world
            .resource::<crate::brush::LastUsedTexture>()
            .texture_path
            .clone();

        let mut new_faces = Vec::with_capacity(hull_faces.len());

        // Map hull vertex indices back to all_local_verts indices
        let hull_to_input: Vec<usize> = hull_positions
            .iter()
            .map(|hp| {
                all_local_verts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (**a - *hp)
                            .length_squared()
                            .partial_cmp(&(**b - *hp).length_squared())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect();

        for hull_face in &hull_faces {
            // Find best matching old face by normal similarity
            let mut best_old = None;
            let mut best_score = -1.0_f32;

            // Check if this face has vertices from the original brush
            let input_verts: Vec<usize> = hull_face
                .vertex_indices
                .iter()
                .map(|&hi| hull_to_input[hi])
                .collect();
            let has_original = input_verts.iter().any(|&i| i < existing_count);

            if has_original {
                for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
                    let old_set: std::collections::HashSet<usize> =
                        old_polygon.iter().copied().collect();
                    let overlap = input_verts
                        .iter()
                        .filter(|&&i| i < existing_count && old_set.contains(&i))
                        .count() as f32;
                    let normal_sim = hull_face.normal.dot(old_brush.faces[old_idx].plane.normal);
                    let score = overlap + normal_sim * 0.1;
                    if score > best_score {
                        best_score = score;
                        best_old = Some(old_idx);
                    }
                }
            }

            let face_data = if let Some(old_idx) = best_old {
                let old_face = &old_brush.faces[old_idx];
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material_index: old_face.material_index,
                    texture_path: old_face.texture_path.clone(),
                    material_name: old_face.material_name.clone(),
                    uv_offset: old_face.uv_offset,
                    uv_scale: old_face.uv_scale,
                    uv_rotation: old_face.uv_rotation,
                }
            } else {
                // New face from the appended shape — use last-used texture
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    texture_path: last_tex.clone(),
                    uv_scale: Vec2::ONE,
                    ..default()
                }
            };
            new_faces.push(face_data);
        }

        let new_brush = Brush { faces: new_faces };

        // Apply
        if let Some(mut brush) = world.get_mut::<Brush>(target_entity) {
            *brush = new_brush.clone();
        }

        // Undo command
        let cmd = crate::brush::SetBrush {
            entity: target_entity,
            old: old_brush,
            new: new_brush,
            label: "Append brush geometry".to_string(),
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(Box::new(cmd));
        history.redo_stack.clear();
    });
}

/// Intersect a ray with a plane defined by a point and normal.
fn ray_plane_intersection(ray: Ray3d, plane_point: Vec3, plane_normal: Vec3) -> Option<Vec3> {
    let denom = ray.direction.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - ray.origin).dot(plane_normal) / denom;
    if t < 0.0 {
        return None;
    }
    Some(ray.origin + *ray.direction * t)
}

/// Draw a grid of small crosses on the drawing plane, centered near `center`.
/// Grid points are world-aligned (fixed at world-space multiples of `inc`),
/// so only the visible window moves with the cursor — individual crosses stay put.
fn draw_plane_grid(
    gizmos: &mut Gizmos,
    plane: &DrawPlane,
    center: Vec3,
    snap_settings: &SnapSettings,
) {
    let inc = snap_settings.grid_size();
    let cross_size = inc * 0.1;
    let range = 10_i32;
    let fade_radius = range as f32 * inc;

    // World-aligned: project center directly onto axes (not relative to plane.origin)
    let u_center = (center.dot(plane.axis_u) / inc).round() as i32;
    let v_center = (center.dot(plane.axis_v) / inc).round() as i32;

    // Distance of the plane from the world origin along its normal
    let plane_d = plane.origin.dot(plane.normal);

    for du in -range..=range {
        for dv in -range..=range {
            let u = (u_center + du) as f32 * inc;
            let v = (v_center + dv) as f32 * inc;
            let pt = plane.axis_u * u + plane.axis_v * v + plane.normal * plane_d;

            // Distance-based alpha fade from cursor
            let dist = (pt - center).length();
            let alpha = (1.0 - dist / fade_radius).clamp(0.0, 0.3);
            if alpha <= 0.0 {
                continue;
            }
            let grid_color = Color::srgba(0.5, 0.5, 0.5, alpha);

            gizmos.line(
                pt - plane.axis_u * cross_size,
                pt + plane.axis_u * cross_size,
                grid_color,
            );
            gizmos.line(
                pt - plane.axis_v * cross_size,
                pt + plane.axis_v * cross_size,
                grid_color,
            );
        }
    }
}

/// Snap a world-space hit point to a world-aligned grid on the drawing plane.
fn snap_to_plane_grid(
    hit: Vec3,
    plane: &DrawPlane,
    snap_settings: &SnapSettings,
    ctrl: bool,
) -> Vec3 {
    if !snap_settings.translate_active(ctrl) || snap_settings.translate_increment <= 0.0 {
        return hit;
    }
    let inc = snap_settings.translate_increment;
    // World-aligned: snap using world-space projections onto axes
    let u = hit.dot(plane.axis_u);
    let v = hit.dot(plane.axis_v);
    let snapped_u = (u / inc).round() * inc;
    let snapped_v = (v / inc).round() * inc;
    let plane_d = plane.origin.dot(plane.normal);
    plane.axis_u * snapped_u + plane.axis_v * snapped_v + plane.normal * plane_d
}

/// Compute the 2D convex hull of coplanar points projected onto the drawing plane.
/// Returns the subset of input points forming the hull, in CCW winding order.
fn convex_hull_on_plane(points: &[Vec3], plane: &DrawPlane) -> Vec<Vec3> {
    if points.len() < 3 {
        return points.to_vec();
    }

    // Project to 2D
    let pts2d: Vec<Vec2> = points
        .iter()
        .map(|p| Vec2::new(p.dot(plane.axis_u), p.dot(plane.axis_v)))
        .collect();

    // Andrew's monotone chain algorithm
    let mut indexed: Vec<usize> = (0..pts2d.len()).collect();
    indexed.sort_by(|&a, &b| {
        pts2d[a]
            .x
            .partial_cmp(&pts2d[b].x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                pts2d[a]
                    .y
                    .partial_cmp(&pts2d[b].y)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    let mut hull: Vec<usize> = Vec::new();
    // Lower hull
    for &i in &indexed {
        while hull.len() >= 2
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    // Upper hull
    let lower_len = hull.len() + 1;
    for &i in indexed.iter().rev() {
        while hull.len() >= lower_len
            && cross(
                pts2d[hull[hull.len() - 2]],
                pts2d[hull[hull.len() - 1]],
                pts2d[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    hull.pop(); // remove duplicate of first point

    hull.iter().map(|&i| points[i]).collect()
}

/// Spawn a brush from polygon vertices + extrude depth.
fn spawn_polygon_brush(active: &ActiveDraw, commands: &mut Commands) {
    if active.polygon_vertices.len() < 3 || active.depth.abs() < MIN_EXTRUDE_DEPTH {
        return;
    }

    let polygon = active.polygon_vertices.clone();
    let normal = active.plane.normal;
    let depth = active.depth;

    commands.queue(move |world: &mut World| {
        // Compute centroid + center
        let centroid: Vec3 = polygon.iter().sum::<Vec3>() / polygon.len() as f32;
        let center = centroid + normal * depth / 2.0;

        // Build rotation: local Y = plane normal
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

        // Convert polygon vertices to local space
        let local_verts: Vec<Vec3> = polygon
            .iter()
            .map(|&v| inv_rotation * (v - center))
            .collect();

        let Some(mut brush) = Brush::prism(&local_verts, Vec3::Y, depth) else {
            return;
        };

        // Apply last-used texture
        let last_tex = world
            .resource::<crate::brush::LastUsedTexture>()
            .texture_path
            .clone();
        if let Some(ref path) = last_tex {
            for face in &mut brush.faces {
                face.texture_path = Some(path.clone());
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

        // Snapshot for undo
        let snapshot = snapshot_entity(world, entity);
        let cmd = CreateBrushCommand {
            entity,
            scene_snapshot: snapshot,
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(Box::new(cmd));
        history.redo_stack.clear();
    });
}

/// Compute the 4 world-space corners of the footprint rectangle.
fn footprint_corners(active: &ActiveDraw) -> [Vec3; 4] {
    let plane = &active.plane;
    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    [
        plane.origin + plane.axis_u * min_u + plane.axis_v * min_v,
        plane.origin + plane.axis_u * max_u + plane.axis_v * min_v,
        plane.origin + plane.axis_u * max_u + plane.axis_v * max_v,
        plane.origin + plane.axis_u * min_u + plane.axis_v * max_v,
    ]
}

/// Build 6 world-space cutter planes from the ActiveDraw cuboid.
fn build_cutter_planes(active: &ActiveDraw) -> Vec<BrushFaceData> {
    let plane = &active.plane;

    let c1_u = (active.corner1 - plane.origin).dot(plane.axis_u);
    let c1_v = (active.corner1 - plane.origin).dot(plane.axis_v);
    let c2_u = (active.corner2 - plane.origin).dot(plane.axis_u);
    let c2_v = (active.corner2 - plane.origin).dot(plane.axis_v);

    let min_u = c1_u.min(c2_u);
    let max_u = c1_u.max(c2_u);
    let min_v = c1_v.min(c2_v);
    let max_v = c1_v.max(c2_v);

    let half_u = (max_u - min_u) / 2.0;
    let half_v = (max_v - min_v) / 2.0;
    let half_depth = active.depth.abs() / 2.0;

    let center_on_plane =
        plane.origin + plane.axis_u * (min_u + max_u) / 2.0 + plane.axis_v * (min_v + max_v) / 2.0;
    let center = center_on_plane + plane.normal * active.depth / 2.0;

    vec![
        // +U face
        BrushFaceData {
            plane: BrushPlane {
                normal: plane.axis_u,
                distance: plane.axis_u.dot(center) + half_u,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
        // -U face
        BrushFaceData {
            plane: BrushPlane {
                normal: -plane.axis_u,
                distance: (-plane.axis_u).dot(center) + half_u,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
        // +V face
        BrushFaceData {
            plane: BrushPlane {
                normal: plane.axis_v,
                distance: plane.axis_v.dot(center) + half_v,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
        // -V face
        BrushFaceData {
            plane: BrushPlane {
                normal: -plane.axis_v,
                distance: (-plane.axis_v).dot(center) + half_v,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
        // +Normal face (depth direction)
        BrushFaceData {
            plane: BrushPlane {
                normal: plane.normal,
                distance: plane.normal.dot(center) + half_depth,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
        // -Normal face
        BrushFaceData {
            plane: BrushPlane {
                normal: -plane.normal,
                distance: (-plane.normal).dot(center) + half_depth,
            },
            uv_scale: Vec2::ONE,
            ..default()
        },
    ]
}

/// Perform CSG subtraction: subtract the drawn cuboid from all intersecting brushes.
fn subtract_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let cutter_planes = build_cutter_planes(active);

    commands.queue(move |world: &mut World| {
        // Phase 1: Collect all brush entities and their data
        let mut query = world.query::<(Entity, &Brush, &GlobalTransform)>();
        let targets: Vec<(Entity, Brush, GlobalTransform)> = query
            .iter(world)
            .map(|(e, b, gt)| (e, b.clone(), *gt))
            .collect();

        // Phase 2: Compute subtractions (pure computation)
        struct SubtractionResult {
            original_entity: Entity,
            fragments: Vec<(Brush, Transform)>,
        }

        let mut results: Vec<SubtractionResult> = Vec::new();

        for (entity, brush, global_transform) in &targets {
            // Transform target planes to world space
            let (_, rotation, translation) = global_transform.to_scale_rotation_translation();
            let world_target = brush_planes_to_world(&brush.faces, rotation, translation);

            // Check intersection
            if !brushes_intersect(&world_target, &cutter_planes) {
                continue;
            }

            // Perform subtraction
            let raw_fragments = subtract_brush(&world_target, &cutter_planes);

            let mut fragment_data: Vec<(Brush, Transform)> = Vec::new();
            for fragment_faces in &raw_fragments {
                // Compute vertices to find centroid (world space)
                let (world_verts, _) = compute_brush_geometry(fragment_faces);
                if world_verts.len() < 4 {
                    continue;
                }
                let centroid: Vec3 = world_verts.iter().sum::<Vec3>() / world_verts.len() as f32;

                // Convert to local space around centroid
                let local_faces: Vec<BrushFaceData> = fragment_faces
                    .iter()
                    .map(|f| BrushFaceData {
                        plane: BrushPlane {
                            normal: f.plane.normal,
                            distance: f.plane.distance - f.plane.normal.dot(centroid),
                        },
                        ..f.clone()
                    })
                    .collect();

                // Clean degenerate faces
                let clean = clean_degenerate_faces(&local_faces);
                if clean.len() < 4 {
                    continue;
                }

                fragment_data.push((
                    Brush { faces: clean },
                    Transform::from_translation(centroid),
                ));
            }

            results.push(SubtractionResult {
                original_entity: *entity,
                fragments: fragment_data,
            });
        }

        if results.is_empty() {
            return;
        }

        // Phase 3: Snapshot originals (just the entity, not children — children are rebuilt
        // automatically by regenerate_brush_meshes when Brush component changes)
        let mut original_snapshots: Vec<(Entity, DynamicScene)> = Vec::new();
        for result in &results {
            let snapshot = DynamicSceneBuilder::from_world(world)
                .extract_entities(std::iter::once(result.original_entity))
                .build();
            original_snapshots.push((result.original_entity, snapshot));
        }

        // Clean up selection: remove originals that are about to be despawned
        {
            let mut selection = world.resource_mut::<Selection>();
            let despawning: Vec<Entity> = original_snapshots.iter().map(|(e, _)| *e).collect();
            selection.entities.retain(|e| !despawning.contains(e));
        }
        for (entity, _) in &original_snapshots {
            if let Ok(mut e) = world.get_entity_mut(*entity) {
                e.remove::<Selected>();
            }
        }

        // Despawn originals
        for (entity, _) in &original_snapshots {
            if let Ok(e) = world.get_entity_mut(*entity) {
                e.despawn();
            }
        }

        // Spawn fragments
        let mut fragment_snapshots: Vec<(Entity, DynamicScene)> = Vec::new();
        for result in &results {
            for (brush, transform) in &result.fragments {
                let entity = world
                    .spawn((
                        Name::new("Brush"),
                        brush.clone(),
                        *transform,
                        Visibility::default(),
                    ))
                    .id();
                let snapshot = DynamicSceneBuilder::from_world(world)
                    .extract_entities(std::iter::once(entity))
                    .build();
                fragment_snapshots.push((entity, snapshot));
            }
        }

        // Push undo command
        let cmd = SubtractBrushCommand {
            originals: original_snapshots,
            fragments: fragment_snapshots,
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(Box::new(cmd));
        history.redo_stack.clear();
    });
}

struct SubtractBrushCommand {
    /// Original brushes to restore on undo (entity + snapshot).
    originals: Vec<(Entity, DynamicScene)>,
    /// Fragment brushes spawned by the subtraction (entity + snapshot).
    fragments: Vec<(Entity, DynamicScene)>,
}

impl EditorCommand for SubtractBrushCommand {
    fn execute(&self, world: &mut World) {
        // Redo: clean up selection, despawn originals, respawn fragments
        {
            let entities: Vec<Entity> = self.originals.iter().map(|(e, _)| *e).collect();
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !entities.contains(e));
        }
        for (entity, _) in &self.originals {
            if let Ok(mut e) = world.get_entity_mut(*entity) {
                e.remove::<Selected>();
            }
        }
        for (entity, _) in &self.originals {
            if let Ok(e) = world.get_entity_mut(*entity) {
                e.despawn();
            }
        }
        for (_, snapshot) in &self.fragments {
            let scene = snapshot_rebuild(snapshot);
            let _ = scene.write_to_world(world, &mut Default::default());
        }
    }

    fn undo(&self, world: &mut World) {
        // Undo: clean up selection, despawn fragments, respawn originals
        {
            let entities: Vec<Entity> = self.fragments.iter().map(|(e, _)| *e).collect();
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !entities.contains(e));
        }
        for (entity, _) in &self.fragments {
            if let Ok(mut e) = world.get_entity_mut(*entity) {
                e.remove::<Selected>();
            }
        }
        for (entity, _) in &self.fragments {
            if let Ok(e) = world.get_entity_mut(*entity) {
                e.despawn();
            }
        }
        for (_, snapshot) in &self.originals {
            let scene = snapshot_rebuild(snapshot);
            let _ = scene.write_to_world(world, &mut Default::default());
        }
    }

    fn description(&self) -> &str {
        "Subtract brush"
    }
}

fn join_selected_brushes(
    keyboard: Res<ButtonInput<KeyCode>>,
    input_focus: Res<InputFocus>,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    brush_query: Query<(&Brush, &GlobalTransform)>,
    mut commands: Commands,
) {
    if !keyboard.just_pressed(KeyCode::KeyJ) {
        return;
    }
    if input_focus.0.is_some() || modal.active.is_some() || draw_state.active.is_some() {
        return;
    }

    // Collect selected brush entities (need at least 2)
    let selected_brushes: Vec<Entity> = selection
        .entities
        .iter()
        .copied()
        .filter(|&e| brush_query.contains(e))
        .collect();
    if selected_brushes.len() < 2 {
        return;
    }

    let primary_entity = selected_brushes[0];
    let others: Vec<Entity> = selected_brushes[1..].to_vec();

    commands.queue(move |world: &mut World| {
        use avian3d::parry::math::Point as ParryPoint;
        use avian3d::parry::transformation::convex_hull;

        // Read primary brush data
        let Some(primary_brush) = world.get::<Brush>(primary_entity) else {
            return;
        };
        let old_primary_brush = primary_brush.clone();

        let Some(primary_gtf) = world.get::<GlobalTransform>(primary_entity) else {
            return;
        };
        let (_, rotation, translation) = primary_gtf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        // Gather all vertices in primary's local space
        let existing_verts = compute_brush_geometry(&old_primary_brush.faces).0;
        let existing_count = existing_verts.len();
        let mut all_local_verts: Vec<Vec3> = existing_verts;

        // Gather vertices from other brushes, converted to primary's local space
        for &other in &others {
            let Some(other_brush) = world.get::<Brush>(other) else {
                continue;
            };
            let Some(other_gtf) = world.get::<GlobalTransform>(other) else {
                continue;
            };
            let (other_verts, _) = compute_brush_geometry(&other_brush.faces);
            for v in &other_verts {
                let world_pos = other_gtf.transform_point(*v);
                all_local_verts.push(inv_rotation * (world_pos - translation));
            }
        }

        if all_local_verts.len() < 4 {
            return;
        }

        // Compute convex hull
        let points: Vec<ParryPoint<f32>> = all_local_verts
            .iter()
            .map(|v| ParryPoint::new(v.x, v.y, v.z))
            .collect();
        let (hull_verts, hull_tris) = convex_hull(&points);
        if hull_verts.len() < 4 || hull_tris.is_empty() {
            return;
        }

        let hull_positions: Vec<Vec3> = hull_verts
            .iter()
            .map(|p| Vec3::new(p.x, p.y, p.z))
            .collect();
        let hull_faces = crate::brush::merge_hull_triangles(&hull_positions, &hull_tris);
        if hull_faces.len() < 4 {
            return;
        }

        // Build new face data, matching old primary faces where possible
        let old_face_polygons = compute_brush_geometry(&old_primary_brush.faces).1;
        let last_tex = world
            .resource::<crate::brush::LastUsedTexture>()
            .texture_path
            .clone();

        let hull_to_input: Vec<usize> = hull_positions
            .iter()
            .map(|hp| {
                all_local_verts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (**a - *hp)
                            .length_squared()
                            .partial_cmp(&(**b - *hp).length_squared())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect();

        let mut new_faces = Vec::with_capacity(hull_faces.len());
        for hull_face in &hull_faces {
            let input_verts: Vec<usize> = hull_face
                .vertex_indices
                .iter()
                .map(|&hi| hull_to_input[hi])
                .collect();
            let has_original = input_verts.iter().any(|&i| i < existing_count);

            let mut best_old = None;
            let mut best_score = -1.0_f32;

            if has_original {
                for (old_idx, old_polygon) in old_face_polygons.iter().enumerate() {
                    let old_set: std::collections::HashSet<usize> =
                        old_polygon.iter().copied().collect();
                    let overlap = input_verts
                        .iter()
                        .filter(|&&i| i < existing_count && old_set.contains(&i))
                        .count() as f32;
                    let normal_sim = hull_face
                        .normal
                        .dot(old_primary_brush.faces[old_idx].plane.normal);
                    let score = overlap + normal_sim * 0.1;
                    if score > best_score {
                        best_score = score;
                        best_old = Some(old_idx);
                    }
                }
            }

            let face_data = if let Some(old_idx) = best_old {
                let old_face = &old_primary_brush.faces[old_idx];
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    material_index: old_face.material_index,
                    texture_path: old_face.texture_path.clone(),
                    material_name: old_face.material_name.clone(),
                    uv_offset: old_face.uv_offset,
                    uv_scale: old_face.uv_scale,
                    uv_rotation: old_face.uv_rotation,
                }
            } else {
                BrushFaceData {
                    plane: BrushPlane {
                        normal: hull_face.normal,
                        distance: hull_face.distance,
                    },
                    texture_path: last_tex.clone(),
                    uv_scale: Vec2::ONE,
                    ..default()
                }
            };
            new_faces.push(face_data);
        }

        let new_brush = Brush { faces: new_faces };

        // Snapshot others before despawning (for undo)
        let mut undo_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

        // SetBrush for primary
        undo_commands.push(Box::new(crate::brush::SetBrush {
            entity: primary_entity,
            old: old_primary_brush,
            new: new_brush.clone(),
            label: "Join brushes".to_string(),
        }));

        // Snapshot and despawn each other brush
        for &other in &others {
            undo_commands.push(Box::new(DespawnEntity::from_world(world, other)));
        }

        // Apply: update primary brush
        if let Some(mut brush) = world.get_mut::<Brush>(primary_entity) {
            *brush = new_brush;
        }

        // Deselect entities before despawning so that `On<Remove, Selected>`
        // observers can clean up tree-row UI while the entities still exist.
        for &other in &others {
            if let Ok(mut ec) = world.get_entity_mut(other) {
                ec.remove::<Selected>();
            }
        }
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !others.contains(e));
        }

        // Despawn others
        for &other in &others {
            if let Ok(entity_mut) = world.get_entity_mut(other) {
                entity_mut.despawn();
            }
        }

        // Push grouped undo command
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(Box::new(CommandGroup {
            commands: undo_commands,
            label: "Join brushes".to_string(),
        }));
        history.redo_stack.clear();
    });
}
