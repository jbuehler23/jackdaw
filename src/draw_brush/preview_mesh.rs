use crate::default_style;
use crate::draw_brush::{
    DrawBrushState, DrawMode, DrawPhase, PUNCH_THROUGH_DEPTH, build_cutter_planes,
    build_cutter_planes_polygon, topology_aabbs_overlap,
};
use crate::{EditorEntity, brush::BrushMaterialPalette, selection::Selected};
use bevy::{
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use jackdaw_geometry::{
    compute_brush_geometry_from_planes, compute_brush_topology, triangulate_face,
};
use jackdaw_jsn::Brush;

#[derive(Component)]
pub(crate) struct DrawPreviewMesh;

#[derive(Component)]
pub(crate) struct CutResultPreviewMesh;

/// Per-face data attached to each [`CutResultPreviewMesh`] so the face-grid
/// gizmo systems can render edges and grid lines on cut-preview fragments.
#[derive(Component)]
pub(crate) struct CutPreviewFace {
    pub world_vertices: Vec<Vec3>,
    pub world_normal: Vec3,
    pub is_default_material: bool,
    pub is_cap: bool,
}

/// Marker on a brush whose render meshes are hidden during cut preview.
#[derive(Component)]
pub(crate) struct CutPreviewHidden;

pub(crate) fn manage_draw_preview_mesh(
    draw_state: Res<DrawBrushState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    preview_query: Query<Entity, With<DrawPreviewMesh>>,
    result_preview_query: Query<Entity, With<CutResultPreviewMesh>>,
    brushes: Query<(Entity, &Brush, &GlobalTransform, Has<Selected>)>,
    hidden_query: Query<Entity, (With<CutPreviewHidden>, With<Brush>)>,
    mut visibility_query: Query<&mut Visibility>,
    palette: Res<BrushMaterialPalette>,
    mut cached_add_material: Local<Option<Handle<StandardMaterial>>>,
    mut cached_cut_material: Local<Option<Handle<StandardMaterial>>>,

    mut cached_preview_key: Local<Option<(Vec3, Vec3, f32, Vec<Vec3>)>>,
) {
    // Show preview for both Add and Cut during extrude phase
    let should_show = draw_state
        .active
        .as_ref()
        .is_some_and(|a| a.phase == DrawPhase::ExtrudingDepth);

    // Despawn existing preview meshes if we shouldn't show
    if !should_show {
        for entity in preview_query.iter() {
            commands.entity(entity).despawn();
        }
        for entity in result_preview_query.iter() {
            commands.entity(entity).despawn();
        }
        // Restore hidden brush faces
        for entity in hidden_query.iter() {
            if let Ok(mut vis) = visibility_query.get_mut(entity) {
                *vis = Visibility::Inherited;
            }
            commands.entity(entity).remove::<CutPreviewHidden>();
        }
        *cached_preview_key = None;
        return;
    }

    let active = draw_state.active.as_ref().unwrap();

    // Cache check: skip rebuild if cutter hasn't changed and preview entities exist
    let current_key = (
        active.corner1,
        active.corner2,
        active.depth,
        active.polygon_vertices.clone(),
    );
    if let Some(ref prev_key) = *cached_preview_key {
        let same = prev_key.0.abs_diff_eq(current_key.0, 1e-6)
            && prev_key.1.abs_diff_eq(current_key.1, 1e-6)
            && (prev_key.2 - current_key.2).abs() < 1e-6
            && prev_key.3.len() == current_key.3.len()
            && prev_key
                .3
                .iter()
                .zip(current_key.3.iter())
                .all(|(a, b)| a.abs_diff_eq(*b, 1e-6));
        if same
            && (active.mode == DrawMode::Cut || !preview_query.is_empty())
            && (active.mode != DrawMode::Cut || !result_preview_query.is_empty())
        {
            return;
        }
    }
    *cached_preview_key = Some(current_key);

    // Build volume planes based on draw type. For Cut mode, force a punch-
    // through depth so the preview matches the actual subtract op (which
    // always extends the cutter through any target).
    let cutter_active_storage;
    let cutter_active = if active.mode == DrawMode::Cut {
        let mut a = active.clone();
        a.depth = -PUNCH_THROUGH_DEPTH;
        cutter_active_storage = a;
        &cutter_active_storage
    } else {
        active
    };
    let cutter_planes = if !cutter_active.polygon_vertices.is_empty() {
        build_cutter_planes_polygon(cutter_active)
    } else {
        build_cutter_planes(cutter_active)
    };

    // Compute mesh geometry from planes
    let (verts, face_polys) = compute_brush_geometry_from_planes(&cutter_planes);
    if verts.len() < 4 {
        for entity in preview_query.iter() {
            commands.entity(entity).despawn();
        }
        for entity in result_preview_query.iter() {
            commands.entity(entity).despawn();
        }
        // Restore hidden brush faces since geometry is invalid
        for entity in hidden_query.iter() {
            if let Ok(mut vis) = visibility_query.get_mut(entity) {
                *vis = Visibility::Inherited;
            }
            commands.entity(entity).remove::<CutPreviewHidden>();
        }
        return;
    }

    // Build triangle mesh from face polygons
    let positions: Vec<[f32; 3]> = verts.iter().map(Vec3::to_array).collect();
    let mut all_indices: Vec<u32> = Vec::new();
    for polygon in &face_polys {
        if polygon.len() < 3 {
            continue;
        }
        let tris = triangulate_face(polygon);
        for tri in &tris {
            all_indices.extend_from_slice(&[tri[0], tri[1], tri[2]]);
        }
    }

    // Compute per-vertex normals by averaging face normals
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for (face_idx, polygon) in face_polys.iter().enumerate() {
        if face_idx < cutter_planes.len() {
            let n = cutter_planes[face_idx].plane.normal.to_array();
            for &vi in polygon {
                normals[vi][0] += n[0];
                normals[vi][1] += n[1];
                normals[vi][2] += n[2];
            }
        }
    }
    for n in &mut normals {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(all_indices));

    // Mode-dependent material color
    let material = match active.mode {
        DrawMode::Add => cached_add_material.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: default_style::DRAW_PREVIEW_MESH,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                perceptual_roughness: 1.0,
                ..default()
            })
        }),
        DrawMode::Cut => cached_cut_material.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: default_style::CUT_PREVIEW_MESH,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                perceptual_roughness: 1.0,
                ..default()
            })
        }),
    };

    // Despawn old preview meshes (do NOT restore hidden faces, they stay hidden while cut is active).
    for entity in preview_query.iter() {
        commands.entity(entity).despawn();
    }
    for entity in result_preview_query.iter() {
        commands.entity(entity).despawn();
    }

    // Spawn solid volume preview for Add mode only.
    if active.mode == DrawMode::Add {
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material.clone()),
            Visibility::Inherited,
            Transform::default(),
            DrawPreviewMesh,
            NotShadowCaster,
            NotShadowReceiver,
            EditorEntity,
        ));
    }

    // In Cut mode, spawn solid result preview meshes for affected brushes.
    // The cutter goes through the mesh-CSG kernel (manifold) so concave
    // targets are handled correctly; the convex-only `subtract_brush` path
    // is intentionally avoided here.
    if active.mode == DrawMode::Cut {
        let cutter_topology = compute_brush_topology(&cutter_planes);
        let cutter_input = jackdaw_csg::CsgInput::new(&cutter_planes, &cutter_topology);

        for (brush_entity, brush, brush_tf, is_selected) in brushes.iter() {
            let (_, rotation, translation) = brush_tf.to_scale_rotation_translation();
            let (world_target_faces, world_target_topo) =
                jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, rotation, translation);

            // Cheap AABB rejection before invoking the kernel. The plane-
            // based separating-axis test isn't sound for concave brushes
            // (a face plane can split the brush's own interior), so we use
            // a topology-vertex AABB overlap instead.
            let intersects = topology_aabbs_overlap(&world_target_topo, &cutter_topology);
            if !intersects {
                if hidden_query.get(brush_entity).is_ok() {
                    if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                        *vis = Visibility::Inherited;
                    }
                    commands.entity(brush_entity).remove::<CutPreviewHidden>();
                }
                continue;
            }

            let target_input = jackdaw_csg::CsgInput::new(&world_target_faces, &world_target_topo);
            let kept_fragments =
                match jackdaw_csg::brush_difference_split(&target_input, &cutter_input) {
                    Ok(pieces) => pieces,
                    Err(jackdaw_csg::CsgError::EmptyResult) => Vec::new(),
                    Err(e) => {
                        warn!("cut preview CSG kernel error: {e}");
                        if hidden_query.get(brush_entity).is_ok() {
                            if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                                *vis = Visibility::Inherited;
                            }
                            commands.entity(brush_entity).remove::<CutPreviewHidden>();
                        }
                        continue;
                    }
                };

            // If the kernel produced no fragments (cutter degenerate /
            // entirely consumed target) keep the original visible. This
            // matches the prior convex-CSG fallback behavior.
            if kept_fragments.is_empty() {
                if hidden_query.get(brush_entity).is_ok() {
                    if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                        *vis = Visibility::Inherited;
                    }
                    commands.entity(brush_entity).remove::<CutPreviewHidden>();
                }
                continue;
            }

            if hidden_query.get(brush_entity).is_err() {
                if let Ok(mut vis) = visibility_query.get_mut(brush_entity) {
                    *vis = Visibility::Hidden;
                }
                commands.entity(brush_entity).insert(CutPreviewHidden);
            }

            for fragment in &kept_fragments {
                if fragment.faces.len() < 4 || fragment.topology.vertices.len() < 4 {
                    continue;
                }
                let frag_verts: Vec<Vec3> = fragment
                    .topology
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .collect();

                for (face_idx, face_data) in fragment.faces.iter().enumerate() {
                    if face_idx >= fragment.topology.polygons.len() {
                        continue;
                    }
                    let indices: Vec<usize> = fragment
                        .topology
                        .face_ring(face_idx)
                        .map(|v| v as usize)
                        .collect();
                    if indices.len() < 3 {
                        continue;
                    }

                    // CSG fragment faces may be concave or keyhole-bridged; the
                    // shared per-face build earcut-triangulates and flat-shades them.
                    let buf = jackdaw_geometry::build_face_render_buffers(
                        &frag_verts,
                        &indices,
                        face_data,
                    );

                    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, buf.positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, buf.normals);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, buf.uvs);
                    mesh.insert_indices(Indices::U32(buf.indices));

                    let material = if face_data.material != Handle::default() {
                        face_data.material.clone()
                    } else if is_selected {
                        palette.default_selected_material.clone()
                    } else {
                        palette.default_material.clone()
                    };

                    let face_world_verts: Vec<Vec3> =
                        indices.iter().map(|&vi| frag_verts[vi]).collect();

                    commands.spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Visibility::Inherited,
                        Transform::default(),
                        CutResultPreviewMesh,
                        CutPreviewFace {
                            world_vertices: face_world_verts,
                            world_normal: face_data.plane.normal,
                            is_default_material: face_data.material == Handle::default(),
                            is_cap: face_data.is_cap,
                        },
                        NotShadowCaster,
                        NotShadowReceiver,
                        EditorEntity,
                    ));
                }
            }
        }
    }
}
