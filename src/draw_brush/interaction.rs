use crate::default_style;
use crate::draw_brush::{
    ConfirmDrawBrushOp, DrawBrushGizmoGroup, DrawBrushState, DrawMode, DrawPhase, DrawPlane,
    EXTRUDE_DEPTH_SENSITIVITY, MIN_FOOTPRINT_SIZE, convex_hull_on_plane, draw_plane_grid,
    footprint_corners, ray_plane_intersection, snap_to_diagonal, snap_to_plane_grid,
};
use crate::prelude::*;
use crate::{
    brush::{BrushMeshCache, BrushMeshChunk},
    snapping::SnapSettings,
    viewport::ViewportCursor,
};
use bevy::{
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
};
use jackdaw_geometry::{compute_brush_geometry_from_planes, compute_face_tangent_axes};
use jackdaw_jsn::Brush;

pub(crate) fn draw_brush_update(
    mut draw_state: ResMut<DrawBrushState>,
    vp: ViewportCursor,
    keyboard: Res<ButtonInput<KeyCode>>,
    snap_settings: Res<SnapSettings>,
    mut ray_cast: MeshRayCast,
    brush_chunks: Query<&BrushMeshChunk>,
    brushes: Query<(&Brush, &GlobalTransform)>,
    brush_caches: Query<&BrushMeshCache>,
) {
    let Some(ref mut active) = draw_state.active else {
        return;
    };

    let Ok(window) = vp.windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    // First frame of an active draw captures the hovered viewport so
    // every subsequent frame stays bound to it; if the captured
    // viewport later disappears we silently fall back to the hover.
    let camera_entity = active.camera.or_else(|| vp.camera_entity());
    let viewport_entity = active.viewport.or_else(|| vp.viewport_entity());
    let (Some(camera_entity), Some(viewport_entity)) = (camera_entity, viewport_entity) else {
        return;
    };
    if active.camera.is_none() {
        active.camera = Some(camera_entity);
    }
    if active.viewport.is_none() {
        active.viewport = Some(viewport_entity);
    }
    let Some((camera, cam_tf)) = vp.camera_for(camera_entity) else {
        return;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return;
    };

    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    active.diagonal_snap = shift;

    match active.phase {
        DrawPhase::PlacingFirstCorner => {
            // Ctrl toggles plane lock
            active.plane_locked = ctrl;

            if !active.plane_locked {
                // Raycast against brush chunk meshes
                let settings =
                    MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
                let hits = ray_cast.cast_ray(ray, &settings);

                let mut best_hit: Option<(Vec3, Vec3)> = None;
                let mut best_distance = f32::MAX;
                let mut best_facing = f32::MIN;

                for (hit_entity, hit_data) in hits {
                    if let Ok(chunk) = brush_chunks.get(*hit_entity)
                        && let Ok((brush, brush_tf)) = brushes.get(chunk.brush_entity)
                    {
                        let Some(tri_idx) = hit_data.triangle_index else {
                            continue;
                        };
                        let Some(&face_idx) = chunk.face_of_tri.get(tri_idx) else {
                            continue;
                        };
                        // `face_of_tri` is evaluated-space; resolve a hit on
                        // a mirrored copy to its authored face so the lookup
                        // into `brush.faces` stays in range. A hit on a
                        // bisect cut cap has no authored face and is skipped.
                        let face_idx = match brush_caches.get(chunk.brush_entity) {
                            Ok(cache) => {
                                let Some(authored) = cache.authored_face(face_idx as usize) else {
                                    continue;
                                };
                                authored
                            }
                            Err(_) => face_idx as usize,
                        };
                        let Some(face) = brush.faces.get(face_idx) else {
                            continue;
                        };
                        let (_, brush_rot, _) = brush_tf.to_scale_rotation_translation();
                        let world_normal = (brush_rot * face.plane.normal).normalize();
                        let camera_facing = (-*ray.direction).dot(world_normal);
                        if camera_facing <= 0.0 {
                            continue;
                        }

                        let dist = hit_data.distance;
                        if dist < best_distance - 0.01 {
                            // Clearly closer, take it.
                            best_hit = Some((hit_data.point, world_normal));
                            best_distance = dist;
                            best_facing = camera_facing;
                        } else if dist < best_distance + 0.01 && camera_facing > best_facing {
                            // Within tolerance, prefer more camera-facing.
                            best_hit = Some((hit_data.point, world_normal));
                            best_facing = camera_facing;
                            best_distance = best_distance.min(dist);
                        }
                    }
                }

                if let Some((hit_point, world_normal)) = best_hit {
                    // Face identified, update plane and cache hit point.
                    active.cached_face_hit = Some(hit_point);
                    let (u, v) = compute_face_tangent_axes(world_normal);
                    let plane = DrawPlane {
                        origin: hit_point,
                        normal: world_normal,
                        axis_u: u,
                        axis_v: v,
                    };
                    let snapped_origin =
                        snap_to_plane_grid(hit_point, &plane, &snap_settings, false);
                    active.plane = DrawPlane {
                        origin: snapped_origin,
                        normal: world_normal,
                        axis_u: u,
                        axis_v: v,
                    };
                } else if active.cached_face_hit.is_some() {
                    // Raycast missed but we recently identified a face.
                    // Project cursor onto cached plane; if still near the face, keep it.
                    if let Some(projected) =
                        ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
                    {
                        let last_hit = active.cached_face_hit.unwrap();
                        let dist = projected.distance(last_hit);
                        if dist > 2.0 {
                            // Cursor has moved well beyond the face, fall back to ground.
                            active.cached_face_hit = None;
                            if let Some(ground_hit) =
                                ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y)
                            {
                                let snapped_origin = snap_settings.snap_translate_vec3(ground_hit);
                                active.plane = DrawPlane {
                                    origin: snapped_origin,
                                    normal: Vec3::Y,
                                    axis_u: Vec3::X,
                                    axis_v: Vec3::Z,
                                };
                            }
                        }
                        // else: keep using cached face plane (cursor still near face)
                    }
                } else {
                    // Never been on a face, fall back to Y=0 ground plane.
                    if let Some(ground_hit) = ray_plane_intersection(ray, Vec3::ZERO, Vec3::Y) {
                        let snapped_origin = snap_settings.snap_translate_vec3(ground_hit);
                        active.plane = DrawPlane {
                            origin: snapped_origin,
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
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                active.cursor_on_plane = Some(snapped);
            }
        }
        DrawPhase::DrawingFootprint => {
            // Project cursor onto the locked drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let mut snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                if shift {
                    snapped = snap_to_diagonal(snapped, active.corner1, &active.plane);
                    snapped = snap_to_plane_grid(snapped, &active.plane, &snap_settings, false);
                }
                active.polygon_vertices.clear();
                active.corner2 = snapped;
            }
        }
        DrawPhase::DrawingRotatedWidth => {
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                let line_vec = active.corner2 - active.corner1;
                let line_dir = line_vec.normalize();
                let axis_perp = active.plane.normal.cross(line_dir).normalize();
                let raw_width = (snapped - active.corner1).dot(axis_perp);
                // Snap width to grid
                let width = if snap_settings.translate_active(ctrl)
                    && snap_settings.translate_increment > 0.0
                {
                    (raw_width / snap_settings.translate_increment).round()
                        * snap_settings.translate_increment
                } else {
                    raw_width
                };
                active.polygon_vertices = vec![
                    active.corner1,
                    active.corner2,
                    active.corner2 + axis_perp * width,
                    active.corner1 + axis_perp * width,
                ];
            }
        }
        DrawPhase::DrawingPolygon => {
            // Project cursor onto drawing plane
            if let Some(hit) = ray_plane_intersection(ray, active.plane.origin, active.plane.normal)
            {
                let mut snapped = snap_to_plane_grid(hit, &active.plane, &snap_settings, false);
                if shift && let Some(&last) = active.polygon_vertices.last() {
                    snapped = snap_to_diagonal(snapped, last, &active.plane);
                    snapped = snap_to_plane_grid(snapped, &active.plane, &snap_settings, false);
                }
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

pub(crate) fn draw_brush_release(
    mouse: Res<ButtonInput<MouseButton>>,
    mut draw_state: ResMut<DrawBrushState>,
    vp: ViewportCursor,
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

    let Ok(window) = vp.windows.single() else {
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let camera_entity = active.camera.or_else(|| vp.camera_entity());
    let viewport_entity = active.viewport.or_else(|| vp.viewport_entity());
    let (Some(camera_entity), Some(viewport_entity)) = (camera_entity, viewport_entity) else {
        return;
    };
    let Some((camera, _)) = vp.camera_for(camera_entity) else {
        return;
    };
    let Some(viewport_cursor) = vp.viewport_cursor_for(camera, viewport_entity, cursor_pos) else {
        return;
    };

    let screen_dist = (cursor_pos - press_pos).length();
    if screen_dist > 5.0 {
        if active.diagonal_snap {
            // Shift+drag: check line length, transition to rotated width phase
            let line_len = (active.corner2 - active.corner1).length();
            if line_len >= MIN_FOOTPRINT_SIZE {
                active.phase = DrawPhase::DrawingRotatedWidth;
            }
        } else {
            // Normal drag: check footprint size, transition to ExtrudingDepth
            let delta = active.corner2 - active.corner1;
            if delta.dot(active.plane.axis_u).abs() >= MIN_FOOTPRINT_SIZE
                && delta.dot(active.plane.axis_v).abs() >= MIN_FOOTPRINT_SIZE
            {
                active.phase = DrawPhase::ExtrudingDepth;
                active.extrude_start_cursor = viewport_cursor;
                active.depth = 0.0;
            }
        }
    } else {
        // Click (no drag): enter polygon mode
        active.phase = DrawPhase::DrawingPolygon;
        active.polygon_vertices = vec![active.corner1];
        active.drag_footprint = false;
    }
    active.press_screen_pos = None;
}

/// Sidecar trigger: an LMB inside an active draw modal dispatches
/// the `draw_brush.confirm` operator. Mouse-button gestures aren't
/// expressible as BEI key actions, so the click-to-operator translation
/// has to live in a system. The operator itself owns the actual
/// phase-advance logic and works for both Add and Cut modes.
pub(crate) fn draw_brush_confirm(
    mouse: Res<ButtonInput<MouseButton>>,
    draw_state: Res<DrawBrushState>,
    mut commands: Commands,
) {
    if !mouse.just_pressed(MouseButton::Left) || draw_state.active.is_none() {
        return;
    }
    commands
        .operator(ConfirmDrawBrushOp::ID)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call();
}

// Enter / Backspace / RMB / Escape during a draw are all handled via
// BEI bindings on the `viewport.draw_brush.*` operators registered in
// `add_to_extension` (and `modal.cancel` for the Add-mode Escape).

pub(crate) fn draw_brush_preview(
    draw_state: Res<DrawBrushState>,
    snap_settings: Res<SnapSettings>,
    mut gizmos: Gizmos<DrawBrushGizmoGroup>,
    brushes: Query<(&Brush, &GlobalTransform)>,
) {
    let Some(ref active) = draw_state.active else {
        return;
    };

    let color = match active.mode {
        DrawMode::Add => default_style::DRAW_MODE,
        DrawMode::Cut => default_style::CUT_MODE,
    };

    // Highlight the append target brush so the user knows they're in hull mode
    if let Some(target) = active.append_target
        && let Ok((brush, brush_tf)) = brushes.get(target)
    {
        let (verts, polys) = compute_brush_geometry_from_planes(&brush.faces);
        for polygon in &polys {
            for i in 0..polygon.len() {
                let a = brush_tf.transform_point(verts[polygon[i]]);
                let b = brush_tf.transform_point(verts[polygon[(i + 1) % polygon.len()]]);
                gizmos.line(a, b, default_style::DRAW_MODE);
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
            if active.diagonal_snap {
                // First, show the line being drawn.
                gizmos.line(active.corner1, active.corner2, color);
                let mid = (active.corner1 + active.corner2) / 2.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            } else {
                // Normal axis-aligned rectangle
                let corners = footprint_corners(active);
                for i in 0..4 {
                    gizmos.line(corners[i], corners[(i + 1) % 4], color);
                }
                let mid = (active.corner1 + active.corner2) / 2.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            }
        }
        DrawPhase::DrawingRotatedWidth => {
            if active.polygon_vertices.len() == 4 {
                for i in 0..4 {
                    gizmos.line(
                        active.polygon_vertices[i],
                        active.polygon_vertices[(i + 1) % 4],
                        color,
                    );
                }
                let mid = active.polygon_vertices.iter().sum::<Vec3>() / 4.0;
                draw_plane_grid(&mut gizmos, &active.plane, mid, &snap_settings);
            } else {
                // Before first mouse move, show just the locked line
                gizmos.line(active.corner1, active.corner2, color);
            }
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
            }

            let grid_center = if !active.polygon_vertices.is_empty() {
                active.polygon_vertices.iter().sum::<Vec3>() / active.polygon_vertices.len() as f32
            } else {
                (active.corner1 + active.corner2) / 2.0
            };
            draw_plane_grid(&mut gizmos, &active.plane, grid_center, &snap_settings);
        }
    }
}
