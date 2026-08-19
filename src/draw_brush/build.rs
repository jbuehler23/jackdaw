use crate::draw_brush::{ActiveDraw, MIN_EXTRUDE_DEPTH, StableIdCounter};
use crate::selection::{Selected, Selection};
use bevy::prelude::*;
use jackdaw_scene_types::Brush;

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

/// Prism solid for the in-progress draw.
pub(crate) fn drawn_brush_from_active(active: &ActiveDraw) -> Option<(Brush, Transform)> {
    let polygon: Vec<Vec3> = if !active.polygon_vertices.is_empty() {
        active.polygon_vertices.clone()
    } else {
        footprint_corners(active).to_vec()
    };
    prism_from_world_polygon(
        &polygon,
        active.plane.normal,
        active.plane.axis_u,
        active.depth,
    )
}

pub(crate) fn spawn_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let Some((mut brush, transform)) = drawn_brush_from_active(active) else {
        return;
    };

    commands.queue(move |world: &mut World| {
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
    let Some((mut drawn_brush, drawn_transform)) = drawn_brush_from_active(active) else {
        return;
    };

    commands.queue(move |world: &mut World| {
        let Some(target_brush) = world.get::<Brush>(target_entity) else {
            return;
        };
        let old_brush = target_brush.clone();

        let Some(global_tf) = world.get::<GlobalTransform>(target_entity) else {
            return;
        };
        let target_affine = global_tf.affine();

        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        if let Some(ref mat) = last_mat {
            for face in &mut drawn_brush.faces {
                face.material = mat.clone();
            }
        }

        let (world_target_faces, world_target_topo) =
            jackdaw_csg::brush_to_world(&old_brush.faces, &old_brush.topology, target_affine);
        let (world_drawn_faces, world_drawn_topo) = jackdaw_csg::brush_to_world(
            &drawn_brush.faces,
            &drawn_brush.topology,
            drawn_transform.compute_affine(),
        );

        let target_input = jackdaw_csg::CsgInput::new(&world_target_faces, &world_target_topo);
        let drawn_input = jackdaw_csg::CsgInput::new(&world_drawn_faces, &world_drawn_topo);
        let unioned = match jackdaw_csg::brush_boolean(
            &target_input,
            &drawn_input,
            jackdaw_csg::BooleanOp::Union,
        ) {
            Ok(brush) => brush,
            Err(e) => {
                warn!("append-to-brush CSG union error: {e}");
                return;
            }
        };
        if unioned.topology.vertices.len() < 4 || unioned.faces.len() < 4 {
            return;
        }

        // Keep the target's transform
        let (local_faces, local_topo) =
            jackdaw_csg::brush_to_world(&unioned.faces, &unioned.topology, target_affine.inverse());

        let new_brush = Brush {
            faces: local_faces,
            topology: local_topo,
        };

        // Apply (ECS + AST). Undo is handled by the enclosing
        // `viewport.draw_brush_modal` operator's snapshot diff; no
        // per-command push needed here.
        crate::brush::sync_brush_to_ast(world, target_entity, &new_brush);
        if let Some(mut brush) = world.get_mut::<Brush>(target_entity) {
            *brush = new_brush;
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
