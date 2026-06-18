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
    reflected_face_plane, triangulate_polygon,
};

pub(super) struct MeshRebuildPlugin;

impl Plugin for MeshRebuildPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                mark_brushes_changed_on_modifier_removal,
                remesh_changed_brushes,
            )
                .chain(),
        );
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
/// Runs on `Changed<Brush>` (which fires both when a brush is first inserted
/// and when its value is mutated in place) and `Changed<ModifierStack>` so live
/// modifier edits re-mesh without a brush touch.
pub fn remesh_changed_brushes(
    mut commands: Commands,
    changed: Query<
        (
            Entity,
            &Brush,
            Option<&jackdaw_geometry::ModifierStack>,
            Option<&Children>,
        ),
        Or<(Changed<Brush>, Changed<jackdaw_geometry::ModifierStack>)>,
    >,
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

    for (entity, brush, stack, children) in &changed {
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
            stack,
            &mut commands,
            &mut meshes,
            &mut materials,
            &assets,
        );
    }
}

/// `remesh_changed_brushes` only reacts to change ticks, so removing a
/// `ModifierStack` would leave the stale evaluated geometry rendered. Touch the
/// `Brush` change tick of affected entities so the next rebuild drops it.
pub fn mark_brushes_changed_on_modifier_removal(
    mut removed: RemovedComponents<jackdaw_geometry::ModifierStack>,
    mut brushes: Query<&mut Brush>,
) {
    for entity in removed.read() {
        if let Ok(mut brush) = brushes.get_mut(entity) {
            brush.set_changed();
        }
    }
}

fn build_brush_meshes(
    entity: Entity,
    brush: &Brush,
    stack: Option<&jackdaw_geometry::ModifierStack>,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
) {
    // Plane-intersection fallback for brushes without authored topology
    // (plane-only legacy data).
    let (vertices, face_polygons) = if !brush.topology.polygons.is_empty() {
        let verts: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
        let polys: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
            .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
            .collect();
        (verts, polys)
    } else {
        compute_brush_geometry_from_planes(&brush.faces)
    };

    // Fold the game-enabled modifiers (the `in_game` entries) over the
    // authored geometry: evaluated copies append after the authored elements.
    // Authored indices are unchanged (identity prefix); face_source maps
    // evaluated face indices back to authored face indices for face-data lookup.
    let game_mods: Vec<&jackdaw_geometry::Modifier> = stack
        .map(|s| {
            s.modifiers
                .iter()
                .filter(|e| e.in_game)
                .map(|e| &e.modifier)
                .collect()
        })
        .unwrap_or_default();
    let (vertices, face_polygons, face_source) = if game_mods.is_empty() {
        (vertices, face_polygons, Vec::new())
    } else {
        let eval = jackdaw_geometry::evaluate_modifier_stack(
            &vertices,
            &face_polygons,
            &brush.faces,
            &game_mods,
        );
        (eval.vertices, eval.face_polygons, eval.face_source)
    };

    // Build the face-data slice to iterate: mirrored faces (entries past
    // the identity prefix, where face_source[i] != i) clone their authored
    // source entry but get their plane recomputed from the evaluated ring,
    // since the authored normal is un-reflected and would wind the
    // triangulation and shade the face inside out. Faces without authored
    // data fall back to default.
    let mirrored_faces: Option<Vec<crate::types::BrushFaceData>> = if face_source.is_empty() {
        None
    } else {
        Some(
            face_source
                .iter()
                .enumerate()
                .map(|(evaluated_idx, &src)| {
                    let mut face = brush.faces.get(src as usize).cloned().unwrap_or_default();
                    if src as usize != evaluated_idx
                        && let Some(plane) =
                            reflected_face_plane(&vertices, &face_polygons[evaluated_idx])
                    {
                        face.plane = plane;
                    }
                    face
                })
                .collect(),
        )
    };
    let evaluated_faces = mirrored_faces.as_deref().unwrap_or(&brush.faces);

    let mut fallback_material: Option<Handle<StandardMaterial>> = None;

    for (face_idx, face_data) in evaluated_faces.iter().enumerate() {
        let indices = &face_polygons[face_idx];
        if indices.len() < 3 {
            continue;
        }

        let (mesh_handle, material) = build_face_mesh(
            &vertices,
            indices,
            face_data,
            meshes,
            materials,
            assets,
            &mut fallback_material,
        );

        commands.spawn((
            crate::DerivedFaceMesh,
            Mesh3d(mesh_handle),
            MeshMaterial3d(material),
            Transform::default(),
            ChildOf(entity),
        ));
    }
}

/// Build one face's `Mesh` (positions/normals/uvs/tangents/indices) and resolve
/// its material handle, falling back to the shared embedded grid texture when
/// the face has no assigned material. `fallback_material` caches the grid
/// material across the per-face loop so it is created at most once per rebuild.
fn build_face_mesh(
    vertices: &[Vec3],
    indices: &[usize],
    face_data: &crate::types::BrushFaceData,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
    fallback_material: &mut Option<Handle<StandardMaterial>>,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    let positions: Vec<[f32; 3]> = indices.iter().map(|&vi| vertices[vi].to_array()).collect();
    let normals: Vec<[f32; 3]> = vec![face_data.plane.normal.to_array(); indices.len()];
    let (u_axis, v_axis) =
        if face_data.uv_u_axis != Vec3::ZERO && face_data.uv_v_axis != Vec3::ZERO {
            (face_data.uv_u_axis, face_data.uv_v_axis)
        } else {
            compute_face_tangent_axes(face_data.plane.normal)
        };
    let uvs = compute_face_uvs(
        vertices,
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

    (mesh_handle, material)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::pbr::StandardMaterial;
    use jackdaw_geometry::{
        BrushFaceData, BrushPlane, MeshMirror, Modifier, ModifierEntry, ModifierStack,
        compute_brush_topology,
    };

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

    /// A half-cube occupying x >= 0: five open faces plus the seam cap at
    /// x=0. With default `MeshMirror` (`mirror_x`, offset=0, `merge_dist`=0.001)
    /// the seam cap welds to itself (no mirrored copy) and the other five
    /// faces each get a mirrored counterpart.
    fn half_cube_brush() -> Brush {
        let hx = 0.5_f32;
        let hy = 0.5_f32;
        let hz = 0.5_f32;
        let make_face = |normal: Vec3, distance: f32| -> BrushFaceData {
            let (u, v) = compute_face_tangent_axes(normal);
            BrushFaceData {
                plane: BrushPlane { normal, distance },
                uv_scale: bevy::math::Vec2::ONE,
                uv_u_axis: u,
                uv_v_axis: v,
                ..default()
            }
        };
        let faces = vec![
            make_face(Vec3::X, hx),
            make_face(Vec3::Y, hy),
            make_face(Vec3::NEG_Y, hy),
            make_face(Vec3::Z, hz),
            make_face(Vec3::NEG_Z, hz),
            // seam cap: normal -X at the mirror plane x=0
            make_face(Vec3::NEG_X, 0.0),
        ];
        let topology = compute_brush_topology(&faces);
        Brush { faces, topology }
    }

    fn mirror_stack() -> ModifierStack {
        ModifierStack {
            modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror::default()))],
        }
    }

    #[test]
    fn mirror_x_half_cube_produces_eleven_face_meshes() {
        let mut app = make_app();

        // 5 authored + 5 mirrored + 1 seam = 11 face-mesh children.
        let brush_entity = app
            .world_mut()
            .spawn((half_cube_brush(), mirror_stack()))
            .id();
        app.update();

        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count, 11,
            "half-cube with a default X-mirror modifier must produce 11 face-mesh children"
        );

        // Mutating the stack alone must re-mesh: Changed<ModifierStack>
        // fires with no brush touch.
        {
            let mut stack = app
                .world_mut()
                .get_mut::<ModifierStack>(brush_entity)
                .expect("stack component exists");
            let Modifier::Mirror(mirror) = &mut stack.modifiers[0].modifier;
            mirror.mirror_x = false;
        }
        app.update();

        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count, 6,
            "disabling the mirror axis must rebuild with authored faces only"
        );

        {
            let mut stack = app
                .world_mut()
                .get_mut::<ModifierStack>(brush_entity)
                .expect("stack component exists");
            let Modifier::Mirror(mirror) = &mut stack.modifiers[0].modifier;
            mirror.mirror_x = true;
        }
        app.update();

        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(count, 11, "re-enabling the mirror axis must re-mesh");

        // Removal alone must drop the mirrored half; no brush touch.
        app.world_mut()
            .entity_mut(brush_entity)
            .remove::<ModifierStack>();
        app.update();

        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count, 6,
            "removing the modifier stack must rebuild with authored faces only"
        );
    }

    #[test]
    fn in_game_disabled_modifier_is_skipped_for_game_mesh() {
        let mut app = make_app();

        // Same X-mirror modifier, but flagged off for the in-game mesh: the
        // game rebuild folds only `in_game` entries, so it must produce the
        // six authored faces with no mirrored half.
        let mut stack = mirror_stack();
        stack.modifiers[0].in_game = false;
        let brush_entity = app.world_mut().spawn((half_cube_brush(), stack)).id();
        app.update();

        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count, 6,
            "an in_game=false modifier must be skipped for the game mesh"
        );
    }

    #[test]
    fn mirrored_cap_face_normals_point_outward() {
        let mut app = make_app();

        let brush_entity = app
            .world_mut()
            .spawn((half_cube_brush(), mirror_stack()))
            .id();
        app.update();

        // The mirrored copy of the +X cap is the only face whose verts all
        // sit at x = -0.5. Its face data clones the authored entry, whose
        // un-reflected +X normal would shade and wind it inside out; the
        // build must recompute the plane from the evaluated ring.
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(brush_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mut found = false;
        for child in children {
            let Some(mesh3d) = app.world().get::<Mesh3d>(child) else {
                continue;
            };
            let mesh = meshes.get(&mesh3d.0).expect("face mesh asset exists");
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .expect("position attribute");
            if !positions.iter().all(|p| (p[0] + 0.5).abs() < 1e-5) {
                continue;
            }
            found = true;
            let normals = mesh
                .attribute(Mesh::ATTRIBUTE_NORMAL)
                .and_then(|a| a.as_float3())
                .expect("normal attribute");
            assert!(
                normals.iter().all(|n| n[0] < 0.0),
                "mirrored -X cap normals must point toward -X, got {normals:?}"
            );
        }
        assert!(found, "a face mesh with all verts at x = -0.5 must exist");
    }
}
