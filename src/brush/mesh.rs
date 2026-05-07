use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings},
    light::{NotShadowCaster, NotShadowReceiver},
    math::Affine2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use super::{BrushFaceEntity, BrushMaterialPalette, BrushMeshCache, BrushPreview};
use crate::default_style;
use crate::draw_brush::DrawBrushState;
use crate::selection::Selected;
use jackdaw_geometry::{
    compute_brush_geometry, compute_face_tangent_axes, compute_face_uvs, triangulate_face,
};

pub(super) struct MeshPlugin;

impl Plugin for MeshPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "../../assets/textures/jd_grid.png");
    }
}

pub(super) fn setup_default_materials(
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut palette: ResMut<BrushMaterialPalette>,
    assets: Res<AssetServer>,
) {
    let defaults = default_style::BRUSH_PALETTE;
    for color in defaults {
        palette.materials.push(materials.add(StandardMaterial {
            base_color: color.with_alpha(1.0),
            ..default()
        }));
        palette
            .preview_materials
            .push(materials.add(StandardMaterial {
                base_color: color.with_alpha(0.75),
                alpha_mode: AlphaMode::Blend,
                ..default()
            }));
    }

    // Create grid-textured default materials with nearest-neighbor sampling
    let grid_handle = load_embedded_asset!(
        &*assets,
        "../../assets/textures/jd_grid.png",
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

    // Tile the 2×2 checker at 0.25 world-unit spacing (matching default grid)
    let uv_tile = Affine2::from_scale(Vec2::splat(2.0));

    palette.default_material = materials.add(StandardMaterial {
        base_color: default_style::DEFAULT_MATERIAL_COLOR,
        base_color_texture: Some(grid_handle.clone()),
        alpha_mode: AlphaMode::Blend,
        uv_transform: uv_tile,
        ..default()
    });
    palette.default_selected_material = materials.add(StandardMaterial {
        base_color: default_style::DEFAULT_MATERIAL_SELECTED_COLOR,
        base_color_texture: Some(grid_handle.clone()),
        alpha_mode: AlphaMode::Blend,
        uv_transform: uv_tile,
        ..default()
    });
}

pub fn regenerate_brush_meshes(
    mut commands: Commands,
    changed_brushes: Query<
        (
            Entity,
            &super::Brush,
            Option<&Children>,
            Option<&super::BrushPreview>,
            Has<Selected>,
        ),
        Changed<super::Brush>,
    >,
    mesh3d_query: Query<(), With<Mesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    palette: Res<BrushMaterialPalette>,
    parents: Query<&ChildOf>,
    selected_query: Query<(), With<Selected>>,
    group_edit: Res<crate::viewport_select::GroupEditState>,
) {
    for (entity, brush, children, preview, is_selected) in &changed_brushes {
        let in_active_group = group_edit
            .active_group
            .is_some_and(|group| parents.get(entity).is_ok_and(|c| c.0 == group));
        let parent_selected = !in_active_group
            && parents
                .get(entity)
                .is_ok_and(|child_of| selected_query.contains(child_of.0));
        let effectively_selected = is_selected || parent_selected;
        // Despawn all Mesh3d children from previous regen cycles.
        if let Some(children) = children {
            for child in children.iter() {
                if mesh3d_query.get(child).is_ok()
                    && let Ok(mut ec) = commands.get_entity(child)
                {
                    ec.despawn();
                }
            }
        }

        let (vertices, face_polygons) = compute_brush_geometry(&brush.faces);

        let mut face_entities = Vec::with_capacity(brush.faces.len());

        for (face_idx, face_data) in brush.faces.iter().enumerate() {
            let indices = &face_polygons[face_idx];
            if indices.len() < 3 {
                face_entities.push(Entity::PLACEHOLDER);
                continue;
            }

            // Build per-face mesh with local vertex positions
            let positions: Vec<[f32; 3]> =
                indices.iter().map(|&vi| vertices[vi].to_array()).collect();
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

            // Fan triangulate: local indices (0..positions.len())
            let local_tris = triangulate_face(&(0..indices.len()).collect::<Vec<_>>());
            let flat_indices: Vec<u32> =
                local_tris.iter().flat_map(|t| t.iter().copied()).collect();

            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
            mesh.insert_indices(Indices::U32(flat_indices));

            let mesh_handle = meshes.add(mesh);

            // Use the face's material handle if set, otherwise fall back to grid default
            let is_default = face_data.material == Handle::default();
            let material = if !is_default {
                face_data.material.clone()
            } else if effectively_selected || preview.is_some() {
                palette.default_selected_material.clone()
            } else {
                palette.default_material.clone()
            };

            let face_entity = commands
                .spawn((
                    BrushFaceEntity {
                        brush_entity: entity,
                        face_index: face_idx,
                    },
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material),
                    Transform::default(),
                    ChildOf(entity),
                    // `BrushFaceEntity` requires `EditorHidden +
                    // NonSerializable`; nothing to insert here.
                ))
                .id();
            if is_default {
                commands
                    .entity(face_entity)
                    .insert((NotShadowCaster, NotShadowReceiver));
            }

            face_entities.push(face_entity);
        }

        commands.entity(entity).insert(BrushMeshCache {
            vertices,
            face_polygons,
            face_entities,
        });
    }
}

/// Reads interaction state each frame and inserts/removes `BrushPreview` on the
/// appropriate brush entity so downstream systems can swap materials.
pub(super) fn sync_brush_preview(
    mut commands: Commands,
    face_drag: Res<super::BrushDragState>,
    vertex_drag: Res<super::VertexDragState>,
    edge_drag: Res<super::EdgeDragState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<super::BrushSelection>,
    existing: Query<Entity, With<BrushPreview>>,
) {
    let preview_entity = if face_drag.active || vertex_drag.active || edge_drag.active {
        selection.entity
    } else if let Some(ref active) = draw_state.active {
        active.append_target
    } else {
        None
    };

    for entity in &existing {
        if Some(entity) != preview_entity {
            commands.entity(entity).remove::<BrushPreview>();
        }
    }

    if let Some(entity) = preview_entity
        && existing.get(entity).is_err()
    {
        commands.entity(entity).insert(BrushPreview);
    }
}

/// Every frame, ensure each brush face entity has the correct default-palette material
/// based on preview / selected state.  Uses direct mutation (no deferred commands) so
/// swaps are visible immediately.
pub(super) fn ensure_brush_face_materials(
    palette: Res<BrushMaterialPalette>,
    brushes: Query<(Entity, &BrushMeshCache, Has<BrushPreview>, Has<Selected>), With<super::Brush>>,
    brush_data: Query<&super::Brush>,
    mut face_mats: Query<(&BrushFaceEntity, &mut MeshMaterial3d<StandardMaterial>)>,
    parents: Query<&ChildOf>,
    selected_query: Query<(), With<Selected>>,
    group_edit: Res<crate::viewport_select::GroupEditState>,
) {
    for (entity, cache, has_preview, is_selected) in &brushes {
        let in_active_group = group_edit
            .active_group
            .is_some_and(|group| parents.get(entity).is_ok_and(|c| c.0 == group));
        let parent_selected = !in_active_group
            && parents
                .get(entity)
                .is_ok_and(|child_of| selected_query.contains(child_of.0));
        let effectively_selected = is_selected || parent_selected;
        let target = if effectively_selected || has_preview {
            &palette.default_selected_material
        } else {
            &palette.default_material
        };
        let Ok(brush) = brush_data.get(entity) else {
            continue;
        };
        for &face_entity in &cache.face_entities {
            if face_entity == Entity::PLACEHOLDER {
                continue;
            }
            let Ok((face, mut mat)) = face_mats.get_mut(face_entity) else {
                continue;
            };
            let Some(face_data) = brush.faces.get(face.face_index) else {
                continue;
            };
            // Only touch faces that use the default palette (no explicit material)
            if face_data.material != Handle::default() {
                continue;
            }
            if mat.0 != *target {
                mat.0 = target.clone();
            }
        }
    }
}
