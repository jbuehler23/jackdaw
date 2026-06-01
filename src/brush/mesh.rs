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
    compute_brush_geometry_from_planes, compute_face_tangent_axes, triangulate_polygon,
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

    // Tile the 2x2 checker at 0.25 world-unit spacing (matching default grid)
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

/// Keep each brush's `Transform.translation` at the geometric centroid
/// of its local vertices. After concave edits (vertex drag, extrude,
/// inset, etc.) the topology vertices drift in local space while the
/// entity Transform stays put, leaving the gizmo (and ray-cast AABB)
/// far from the visible mesh. This system shifts the local vertices
/// back so their centroid is at the origin, then translates the entity
/// Transform by the equivalent world-space offset so the rendered
/// position stays the same.
///
/// Skipped while a vertex / edge / face drag or the edit-mode gizmo drag
/// is active so mid-drag world coordinates remain stable.
pub fn recenter_brush_origins(
    mut brushes: Query<
        (
            &mut super::Brush,
            &mut Transform,
            Option<&mut crate::brush::BrushHalfedge>,
        ),
        Or<(Changed<super::Brush>, Changed<crate::brush::BrushHalfedge>)>,
    >,
    vertex_drag: Res<super::VertexDragState>,
    edge_drag: Res<super::EdgeDragState>,
    face_drag: Res<super::BrushDragState>,
    edit_gizmo_drag: Res<crate::gizmos::EditGizmoDragState>,
) {
    if vertex_drag.active || edge_drag.active || face_drag.active || edit_gizmo_drag.active {
        return;
    }
    for (mut brush, mut transform, halfedge) in &mut brushes {
        // Prefer the live halfedge mesh when present (Vertex / Edge /
        // Face mode), since `brush.topology` may not yet reflect the
        // in-flight halfedge edits.
        let verts: Vec<bevy::math::Vec3> = if let Some(ref he) = halfedge {
            he.mesh.verts.values().map(|v| v.co).collect()
        } else {
            brush.topology.vertices.iter().map(|v| v.position).collect()
        };
        if verts.is_empty() {
            continue;
        }
        let centroid = verts.iter().copied().sum::<bevy::math::Vec3>() / verts.len() as f32;
        // Skip when the centroid drift is too small to matter; this
        // also stops the system from re-triggering itself once the
        // brush is centered.
        if centroid.length_squared() < 1e-6 {
            continue;
        }
        for v in &mut brush.topology.vertices {
            v.position -= centroid;
        }
        if let Some(mut he) = halfedge {
            for (_, vert) in he.mesh.verts.iter_mut() {
                vert.co -= centroid;
            }
        }
        let world_offset = transform.rotation * (centroid * transform.scale);
        transform.translation += world_offset;
    }
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
        Or<(Changed<super::Brush>, Changed<crate::brush::BrushHalfedge>)>,
    >,
    mesh3d_query: Query<(), With<Mesh3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    palette: Res<BrushMaterialPalette>,
    parents: Query<&ChildOf>,
    selected_query: Query<(), With<Selected>>,
    group_edit: Res<crate::viewport_select::GroupEditState>,
    halfedge_q: Query<&crate::brush::BrushHalfedge>,
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

        let (vertices, face_polygons) = if let Ok(halfedge) = halfedge_q.get(entity) {
            // In Vertex/Edge/Face edit mode the HalfedgeMesh holds the live
            // post-op topology; flatten it so previews track in-flight edits.
            let topology = halfedge.mesh.flatten_to_topology();
            let verts: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
            let polys: Vec<Vec<usize>> = (0..topology.polygons.len())
                .map(|i| topology.face_ring(i).map(|v| v as usize).collect())
                .collect();
            (verts, polys)
        } else if !brush.topology.polygons.is_empty() {
            // Out of edit mode (or for legacy brushes that were migrated
            // already): read straight from `brush.topology`. The
            // plane-intersection path below only handles convex brushes
            // and silently distorts non-convex / chamfered faces, so we
            // prefer the authored ring whenever it exists. The fallback
            // is kept as a safety net for malformed / empty brushes.
            let verts: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
            let polys: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
                .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
                .collect();
            (verts, polys)
        } else {
            compute_brush_geometry_from_planes(&brush.faces)
        };

        let mut face_entities = Vec::with_capacity(brush.faces.len());

        for (face_idx, face_data) in brush.faces.iter().enumerate() {
            let indices = &face_polygons[face_idx];
            if indices.len() < 3 {
                face_entities.push(Entity::PLACEHOLDER);
                continue;
            }

            // Build per-triangle (flat-shaded) mesh so non-planar faces render correctly.
            // Each triangle in the fan gets its own computed normal; vertex positions are
            // duplicated (3 per tri) so every vertex can carry an independent normal.
            let (u_axis, v_axis) =
                if face_data.uv_u_axis != Vec3::ZERO && face_data.uv_v_axis != Vec3::ZERO {
                    (face_data.uv_u_axis, face_data.uv_v_axis)
                } else {
                    compute_face_tangent_axes(face_data.plane.normal)
                };

            // Concave / annulus-aware triangulation via earcut. Fan
            // triangulation would silently mis-triangulate concave faces
            // and fill keyhole-bridged holes with bogus geometry.
            let ring_u32: Vec<u32> = indices.iter().map(|&i| i as u32).collect();
            let tris = triangulate_polygon(&vertices, &ring_u32, face_data.plane.normal);

            let mut positions: Vec<[f32; 3]> = Vec::with_capacity(tris.len() * 3);
            let mut normals: Vec<[f32; 3]> = Vec::with_capacity(tris.len() * 3);
            let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(tris.len() * 3);
            let mut tangents: Vec<[f32; 4]> = Vec::with_capacity(tris.len() * 3);
            let mut tri_indices: Vec<u32> = Vec::with_capacity(tris.len() * 3);

            let cos_r = face_data.uv_rotation.cos();
            let sin_r = face_data.uv_rotation.sin();

            for tri in &tris {
                let p_a = vertices[tri[0] as usize];
                let p_b = vertices[tri[1] as usize];
                let p_c = vertices[tri[2] as usize];

                // Compute this triangle's actual normal for flat shading.
                let cross = (p_b - p_a).cross(p_c - p_a);
                let tri_normal = if cross.length_squared() > 1e-10 {
                    cross.normalize()
                } else {
                    face_data.plane.normal
                };
                let tri_normal_arr = tri_normal.to_array();

                // Tangent sign uses the face u/v axes (UV continuity is per-face).
                let w = tri_normal.dot(u_axis.cross(v_axis)).signum();
                let tangent = [u_axis.x, u_axis.y, u_axis.z, w];

                let base = tri_indices.len() as u32;
                for &vert_pos in &[p_a, p_b, p_c] {
                    positions.push(vert_pos.to_array());
                    normals.push(tri_normal_arr);

                    // UV math matches compute_face_uvs exactly:
                    // project -> rotate -> scale -> offset.
                    let u = vert_pos.dot(u_axis);
                    let v = vert_pos.dot(v_axis);
                    let ru = u * cos_r - v * sin_r;
                    let rv = u * sin_r + v * cos_r;
                    let su = ru / face_data.uv_scale.x.max(0.001) + face_data.uv_offset.x;
                    let sv = rv / face_data.uv_scale.y.max(0.001) + face_data.uv_offset.y;
                    uvs.push([su, sv]);
                    tangents.push(tangent);
                }
                tri_indices.push(base);
                tri_indices.push(base + 1);
                tri_indices.push(base + 2);
            }

            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
            mesh.insert_indices(Indices::U32(tri_indices));

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
        selection.active_brush
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
