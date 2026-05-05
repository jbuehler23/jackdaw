use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings},
    math::Affine2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::types::Brush;
use jackdaw_geometry::{
    compute_brush_geometry, compute_face_tangent_axes, compute_face_uvs, triangulate_face,
};

pub(super) struct MeshRebuildPlugin;

impl Plugin for MeshRebuildPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(rebuild_brush_meshes);
        embedded_asset!(app, "../assets/jd_grid.png");
    }
}

/// Simplified runtime mesh rebuild for consumers (no editor material palette,
/// no `BrushFaceEntity`, no texture cache, just a single mesh child per brush).
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

    let (vertices, face_polygons) = compute_brush_geometry(&brush.faces);
    let mut all_positions: Vec<[f32; 3]> = Vec::new();
    let mut all_normals: Vec<[f32; 3]> = Vec::new();
    let mut all_uvs: Vec<[f32; 2]> = Vec::new();
    let mut all_tangents: Vec<[f32; 4]> = Vec::new();
    let mut all_indices: Vec<u32> = Vec::new();

    for (face_idx, face_data) in brush.faces.iter().enumerate() {
        let indices = &face_polygons[face_idx];
        if indices.len() < 3 {
            continue;
        }

        let base_vertex = all_positions.len() as u32;

        // Per-face vertices (duplicated for flat normals)
        for &vi in indices {
            all_positions.push(vertices[vi].to_array());
            all_normals.push(face_data.plane.normal.to_array());
        }

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
        all_uvs.extend_from_slice(&uvs);
        let w = face_data.plane.normal.dot(u_axis.cross(v_axis)).signum();
        let tangent = [u_axis.x, u_axis.y, u_axis.z, w];
        all_tangents.extend(std::iter::repeat_n(tangent, indices.len()));

        // Fan triangulate with local indices
        let local_indices: Vec<usize> = (0..indices.len()).collect();
        let tris = triangulate_face(&local_indices);
        for tri in &tris {
            all_indices.push(base_vertex + tri[0]);
            all_indices.push(base_vertex + tri[1]);
            all_indices.push(base_vertex + tri[2]);
        }
    }

    if all_positions.is_empty() {
        return;
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, all_positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, all_normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, all_uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, all_tangents);
    mesh.insert_indices(Indices::U32(all_indices));

    let mesh_handle = meshes.add(mesh);

    let grid_handle = load_embedded_asset!(
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

    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(grid_handle),
        alpha_mode: AlphaMode::Opaque,
        uv_transform: Affine2::from_scale(Vec2::splat(2.0)),
        ..default()
    });

    let child = commands
        .spawn((
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::default(),
            ChildOf(entity),
        ))
        .id();
    let _ = child;
}
