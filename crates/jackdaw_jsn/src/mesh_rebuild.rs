use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings},
    math::Affine2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::types::Brush;
use jackdaw_geometry::{
    compute_brush_geometry_from_planes, compute_face_tangent_axes, compute_face_uvs,
    triangulate_polygon,
};

pub(super) struct MeshRebuildPlugin;

impl Plugin for MeshRebuildPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, remesh_changed_brushes);
        embedded_asset!(app, "../assets/jd_grid.png");
    }
}

/// Runtime brush rebuild. Builds one mesh + child entity per face so each
/// face can carry its own `StandardMaterial` (from `BrushFaceData.material`,
/// typically a catalog `@Name` reference). Faces with an unset handle fall
/// back to the embedded grid texture so brushes still render before any
/// material is assigned.
///
/// Prefers `brush.topology` for face vertex positions (so concave / beveled
/// brushes render with the exact rings authored by edit-mesh ops). Falls
/// back to plane intersection only for legacy brushes whose `.jsn` files
/// pre-date the topology field - that path is convex-only and silently
/// distorts non-convex faces.
///
/// Runs on `Changed<Brush>`, which fires both when a brush is first inserted
/// and when its value is mutated in place. The clear-then-rebuild pass is
/// idempotent: existing face-mesh children (those with `Mesh3d`) are despawned
/// before the new ones are built, so there is no double-mesh on the insert frame.
pub fn remesh_changed_brushes(
    mut commands: Commands,
    changed: Query<(Entity, &Brush, Option<&Children>), Changed<Brush>>,
    face_meshes: Query<(), With<Mesh3d>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    assets: Res<AssetServer>,
) {
    // A headless runtime (a dedicated server) compiles with `render` for the
    // scene types but adds no rendering plugins, so the mesh and material asset
    // stores are absent. Keep the loaded `Brush` component, but skip mesh
    // generation; nothing renders it there.
    let (Some(mut meshes), Some(mut materials)) = (meshes, materials) else {
        return;
    };

    for (entity, brush, children) in &changed {
        // Clear existing face-mesh children so re-runs are idempotent and a
        // mutated brush does not accumulate stale face entities.
        if let Some(children) = children {
            for &child in children {
                if face_meshes.get(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }

        build_brush_meshes(
            entity,
            brush,
            &mut commands,
            &mut meshes,
            &mut materials,
            &assets,
        );
    }
}

fn build_brush_meshes(
    entity: Entity,
    brush: &Brush,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
) {
    // Plane-intersection fallback covers the runtime / preview path
    // where the editor's migration system has not yet run.
    let (vertices, face_polygons) = if !brush.topology.polygons.is_empty() {
        let verts: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
        let polys: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
            .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
            .collect();
        (verts, polys)
    } else {
        compute_brush_geometry_from_planes(&brush.faces)
    };
    let mut fallback_material: Option<Handle<StandardMaterial>> = None;

    for (face_idx, face_data) in brush.faces.iter().enumerate() {
        let indices = &face_polygons[face_idx];
        if indices.len() < 3 {
            continue;
        }

        let positions: Vec<[f32; 3]> = indices.iter().map(|&vi| vertices[vi].to_array()).collect();
        let normals: Vec<[f32; 3]> = vec![face_data.plane.normal.to_array(); indices.len()];
        let (u_axis, v_axis) =
            if face_data.uv_u_axis != Vec3::ZERO && face_data.uv_v_axis != Vec3::ZERO {
                (face_data.uv_u_axis, face_data.uv_v_axis)
            } else {
                compute_face_tangent_axes(face_data.plane.normal)
            };
        let uvs = compute_face_uvs(
            &vertices,
            indices,
            u_axis,
            v_axis,
            face_data.uv_offset,
            face_data.uv_scale,
            face_data.uv_rotation,
        );
        let w = face_data.plane.normal.dot(u_axis.cross(v_axis)).signum();
        let tangent = [u_axis.x, u_axis.y, u_axis.z, w];
        let tangents: Vec<[f32; 4]> = vec![tangent; indices.len()];

        // Concave / keyhole-bridged faces need a real triangulator; fan
        // triangulation would fill holes and mis-tile L-shapes.
        let face_verts_3d: Vec<Vec3> = indices.iter().map(|&vi| vertices[vi]).collect();
        let identity_ring: Vec<u32> = (0..indices.len() as u32).collect();
        let local_tris =
            triangulate_polygon(&face_verts_3d, &identity_ring, face_data.plane.normal);
        let flat_indices: Vec<u32> = local_tris.iter().flat_map(|t| t.iter().copied()).collect();

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        mesh.insert_indices(Indices::U32(flat_indices));
        let mesh_handle = meshes.add(mesh);

        let material = if face_data.material != Handle::default() {
            face_data.material.clone()
        } else {
            fallback_material
                .get_or_insert_with(|| {
                    let grid = load_embedded_asset!(
                        assets,
                        "../assets/jd_grid.png",
                        |settings: &mut ImageLoaderSettings| {
                            let sampler = settings.sampler.get_or_init_descriptor();
                            sampler.mag_filter = ImageFilterMode::Nearest;
                            sampler.min_filter = ImageFilterMode::Nearest;
                            sampler.mipmap_filter = ImageFilterMode::Nearest;
                            sampler.address_mode_u = ImageAddressMode::Repeat;
                            sampler.address_mode_v = ImageAddressMode::Repeat;
                            sampler.address_mode_w = ImageAddressMode::Repeat;
                        }
                    );
                    materials.add(StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: Some(grid),
                        alpha_mode: AlphaMode::Opaque,
                        uv_transform: Affine2::from_scale(Vec2::splat(2.0)),
                        ..default()
                    })
                })
                .clone()
        };

        commands.spawn((
            crate::DerivedFaceMesh,
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::default(),
            ChildOf(entity),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::pbr::StandardMaterial;

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        app.add_plugins(ImagePlugin::default());
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.add_plugins(MeshRebuildPlugin);
        app
    }

    fn face_mesh_child_count(app: &mut App, brush_entity: Entity) -> usize {
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(brush_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        children
            .iter()
            .filter(|&&child| app.world().get::<Mesh3d>(child).is_some())
            .count()
    }

    #[test]
    fn changed_brush_remeshes_on_mutation() {
        let mut app = make_app();

        // Spawn a cuboid brush with 6 faces; the insert frame runs the Changed
        // system and meshes it.
        let brush_entity = app.world_mut().spawn(Brush::cuboid(0.5, 0.5, 0.5)).id();
        app.update();

        let count_after_insert = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count_after_insert, 6,
            "cuboid must produce exactly 6 face-mesh children on insert"
        );

        // Mutate the brush via get_mut. This does NOT re-insert, so the old
        // insert observer (had it been left in place) would never fire.
        // Changed<Brush> must detect the mutation and rebuild.
        {
            let mut brush = app
                .world_mut()
                .get_mut::<Brush>(brush_entity)
                .expect("brush entity exists");
            // Extend to 7 faces by duplicating the last face.
            let extra = brush.faces.last().cloned().expect("at least one face");
            brush.faces.push(extra);
            // Also keep topology in sync so the topology path is used.
            let extra_poly = brush.topology.polygons.last().cloned().expect("poly");
            brush.topology.polygons.push(extra_poly);
        }

        app.update();

        let count_after_mutation = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count_after_mutation, 7,
            "mutated brush must produce 7 face-mesh children; old children must be cleared"
        );
    }

    #[test]
    fn insert_frame_meshes_exactly_once() {
        let mut app = make_app();

        let brush_entity = app.world_mut().spawn(Brush::cuboid(0.5, 0.5, 0.5)).id();
        app.update();

        // No duplicate children: the clear-then-rebuild pass on the insert
        // frame must not produce double the expected face count.
        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(count, 6, "cuboid must produce exactly 6 children, not 12");
    }
}
