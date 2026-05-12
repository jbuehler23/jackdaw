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
    triangulate_face,
};

pub(super) struct MeshRebuildPlugin;

impl Plugin for MeshRebuildPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(rebuild_brush_meshes);
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
/// pre-date the topology field — that path is convex-only and silently
/// distorts non-convex faces.
pub fn rebuild_brush_meshes(
    insert: On<Insert, Brush>,
    mut commands: Commands,
    new_brushes: Query<(Entity, &Brush)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let Ok((entity, brush)) = new_brushes.get(insert.entity) else {
        return;
    };

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

        let local_tris = triangulate_face(&(0..indices.len()).collect::<Vec<_>>());
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
                        &*assets,
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
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::default(),
            ChildOf(entity),
        ));
    }
}
