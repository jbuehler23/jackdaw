use crate::draw_brush::{ActiveDraw, MIN_EXTRUDE_DEPTH, StableIdCounter};
use crate::selection::{Selected, Selection};
use bevy::prelude::*;
use jackdaw_geometry::{
    compute_brush_geometry_from_planes, compute_brush_topology, compute_face_tangent_axes,
};
use jackdaw_scene_types::{Brush, BrushFaceData, BrushPlane};

/// Rotation that maps local X -> `axis_u`, local Y -> `normal`,
/// local Z -> `axis_u` × `normal` (right-handed).
pub(crate) fn rotation_from_draw_axes(normal: Vec3, axis_u: Vec3) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(axis_u, normal, axis_u.cross(normal)))
}

/// Build a local-space prism and it's transform.
pub(crate) fn prism_from_world_polygon(
    polygon: &[Vec3],
    normal: Vec3,
    axis_u: Vec3,
    depth: f32,
) -> Option<(Brush, Transform)> {
    if polygon.len() < 3 || depth.abs() < MIN_EXTRUDE_DEPTH {
        return None;
    }

    let centroid = polygon.iter().copied().sum::<Vec3>() / polygon.len() as f32;
    let center = centroid + normal * depth / 2.0;
    let rotation = rotation_from_draw_axes(normal, axis_u);
    let inv_rotation = rotation.inverse();
    let local_verts: Vec<Vec3> = polygon
        .iter()
        .map(|&vertex| inv_rotation * (vertex - centroid))
        .collect();
    let brush = Brush::prism(&local_verts, Vec3::Y, depth)?;
    Some((
        brush,
        Transform {
            translation: center,
            rotation,
            scale: Vec3::ONE,
        },
    ))
}

pub(crate) fn spawn_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let polygon: Vec<Vec3> = if !active.polygon_vertices.is_empty() {
        active.polygon_vertices.clone()
    } else {
        footprint_corners(active).to_vec()
    };
    let plane = active.plane.clone();
    let depth = active.depth;

    commands.queue(move |world: &mut World| {
        let Some((mut brush, transform)) =
            prism_from_world_polygon(&polygon, plane.normal, plane.axis_u, depth)
        else {
            return;
        };

        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(ref mat) = last_mat {
            for face in &mut brush.faces {
                face.material = mat.clone();
            }
        }

        let stable_id = world.resource_mut::<StableIdCounter>().next();
        let entity = world
            .spawn((
                Name::new("Brush"),
                brush,
                transform,
                Visibility::default(),
                stable_id,
            ))
            .id();

        crate::scene_io::register_entity_in_ast(world, entity);

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
    });
}

pub(crate) fn append_to_brush(active: &ActiveDraw, commands: &mut Commands) {
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
        let Some(brush) = world.get::<Brush>(target_entity) else {
            return;
        };
        let old_brush = brush.clone();

        let Some(global_tf) = world.get::<GlobalTransform>(target_entity) else {
            return;
        };
        let (_, rotation, translation) = global_tf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        let existing_verts = compute_brush_geometry_from_planes(&old_brush.faces).0;
        let existing_count = existing_verts.len();

        let mut all_local_verts: Vec<Vec3> = existing_verts;
        for v in &drawn_verts {
            all_local_verts.push(inv_rotation * (*v - translation));
        }

        if all_local_verts.len() < 4 {
            return;
        }

        let old_face_polygons = compute_brush_geometry_from_planes(&old_brush.faces).1;
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        let Some(new_faces) = jackdaw_hull::build_hull_faces_matching(
            &all_local_verts,
            existing_count,
            &old_brush.faces,
            &old_face_polygons,
            last_mat.unwrap_or_default(),
        ) else {
            return;
        };

        let topology = compute_brush_topology(&new_faces);
        let new_brush = Brush {
            faces: new_faces,
            topology,
        };

        // Apply (ECS + AST). Undo is handled by the enclosing
        // `viewport.draw_brush_modal` operator's snapshot diff; no
        // per-command push needed here.
        crate::brush::sync_brush_to_ast(world, target_entity, &new_brush);
        if let Some(mut brush) = world.get_mut::<Brush>(target_entity) {
            *brush = new_brush.clone();
        }
    });
}

/// Compute the 4 world-space corners of the footprint rectangle.
pub(crate) fn footprint_corners(active: &ActiveDraw) -> [Vec3; 4] {
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

/// Build 6 world-space cutter planes from the `ActiveDraw` cuboid.
pub(crate) fn build_cutter_planes(active: &ActiveDraw) -> Vec<BrushFaceData> {
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

    let normals_dists = [
        (plane.axis_u, plane.axis_u.dot(center) + half_u),
        (-plane.axis_u, (-plane.axis_u).dot(center) + half_u),
        (plane.axis_v, plane.axis_v.dot(center) + half_v),
        (-plane.axis_v, (-plane.axis_v).dot(center) + half_v),
        (plane.normal, plane.normal.dot(center) + half_depth),
        (-plane.normal, (-plane.normal).dot(center) + half_depth),
    ];
    normals_dists
        .iter()
        .map(|&(normal, distance)| {
            let (u, v) = compute_face_tangent_axes(normal);
            BrushFaceData {
                plane: BrushPlane { normal, distance },
                uv_scale: Vec2::ONE,
                uv_u_axis: u,
                uv_v_axis: v,
                ..default()
            }
        })
        .collect()
}

/// Build N+2 world-space cutter planes from a polygon prism `ActiveDraw`.
pub(crate) fn build_cutter_planes_polygon(active: &ActiveDraw) -> Vec<BrushFaceData> {
    let verts = &active.polygon_vertices;
    let normal = active.plane.normal;
    let depth = active.depth;
    let half_depth = depth.abs() / 2.0;
    let centroid: Vec3 = verts.iter().sum::<Vec3>() / verts.len() as f32;
    let center = centroid + normal * depth / 2.0;

    let mut faces = Vec::new();

    // Top cap (+normal)
    let (top_u, top_v) = compute_face_tangent_axes(normal);
    faces.push(BrushFaceData {
        plane: BrushPlane {
            normal,
            distance: normal.dot(center) + half_depth,
        },
        uv_scale: Vec2::ONE,
        uv_u_axis: top_u,
        uv_v_axis: top_v,
        ..default()
    });

    // Bottom cap (-normal)
    let (bot_u, bot_v) = compute_face_tangent_axes(-normal);
    faces.push(BrushFaceData {
        plane: BrushPlane {
            normal: -normal,
            distance: (-normal).dot(center) + half_depth,
        },
        uv_scale: Vec2::ONE,
        uv_u_axis: bot_u,
        uv_v_axis: bot_v,
        ..default()
    });

    // Side planes: one per polygon edge
    let n = verts.len();
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let edge = b - a;
        let mut side_normal = edge.cross(normal).normalize_or_zero();
        if side_normal.length_squared() < 0.5 {
            continue;
        }
        // Ensure outward-facing
        if side_normal.dot(a - centroid) < 0.0 {
            side_normal = -side_normal;
        }
        let distance = side_normal.dot(a);
        let (su, sv) = compute_face_tangent_axes(side_normal);
        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal: side_normal,
                distance,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: su,
            uv_v_axis: sv,
            ..default()
        });
    }

    faces
}
