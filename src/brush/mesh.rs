use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings},
    light::{NotShadowCaster, NotShadowReceiver},
    math::Affine2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use super::{BrushMaterialPalette, BrushMeshCache, BrushPreview};
use crate::default_style;
use crate::draw_brush::DrawBrushState;
use crate::selection::Selected;
use jackdaw_geometry::compute_brush_geometry_from_planes;

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

        let chunks = super::mesh_chunks::build_mesh_chunks(&vertices, &face_polygons, &brush.faces);
        let mut chunk_entities = Vec::with_capacity(chunks.len());

        for chunk in chunks {
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, chunk.positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, chunk.normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, chunk.uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, chunk.tangents);
            mesh.insert_indices(Indices::U32(chunk.indices));
            let mesh_handle = meshes.add(mesh);

            // Explicit face material, or the palette default with the
            // selection/preview variant applied at build time.
            let material = if !chunk.uses_default_material {
                chunk.material.clone()
            } else if effectively_selected || preview.is_some() {
                palette.default_selected_material.clone()
            } else {
                palette.default_material.clone()
            };

            let chunk_entity = commands
                .spawn((
                    super::BrushMeshChunk {
                        brush_entity: entity,
                        face_of_tri: chunk.face_of_tri,
                        uses_default_material: chunk.uses_default_material,
                    },
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material),
                    Transform::default(),
                    ChildOf(entity),
                ))
                .id();
            if chunk.uses_default_material {
                commands
                    .entity(chunk_entity)
                    .insert((NotShadowCaster, NotShadowReceiver));
            }
            chunk_entities.push(chunk_entity);
        }

        commands.entity(entity).insert(BrushMeshCache {
            vertices,
            face_polygons,
            chunk_entities,
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

/// Every frame, ensure each brush mesh chunk has the correct default-palette
/// material based on preview / selected state. Uses direct mutation (no
/// deferred commands) so swaps are visible immediately. Chunks with explicit
/// face materials are never touched.
pub(super) fn ensure_brush_chunk_materials(
    palette: Res<BrushMaterialPalette>,
    brushes: Query<(Entity, &BrushMeshCache, Has<BrushPreview>, Has<Selected>), With<super::Brush>>,
    mut chunk_mats: Query<(
        &super::BrushMeshChunk,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
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
        for &chunk_entity in &cache.chunk_entities {
            let Ok((chunk, mut mat)) = chunk_mats.get_mut(chunk_entity) else {
                continue;
            };
            if !chunk.uses_default_material {
                continue;
            }
            if mat.0 != *target {
                mat.0 = target.clone();
            }
        }
    }
}
