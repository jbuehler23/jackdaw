use bevy::{
    asset::{embedded_asset, load_embedded_asset},
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings},
    math::Affine2,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::types::Brush;
use jackdaw_geometry::compute_brush_geometry_from_planes;

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

/// Runtime brush rebuild. Meshes a brush into one mesh + child entity per
/// material chunk: faces are grouped by their `StandardMaterial` (from
/// `BrushFaceData.material`, typically a catalog `@Name` reference). Faces with
/// an unset handle fall back to the embedded grid texture so brushes still
/// render before any material is assigned.
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

    // Resolve the evaluated face data (mirrored polygons get their plane
    // recomputed from the reflected ring) and mesh it into per-material chunks.
    // `build_brush_chunks` is the shared editor / runtime build.
    let evaluated_faces = if face_source.is_empty() {
        brush.faces.clone()
    } else {
        jackdaw_geometry::resolve_evaluated_faces(
            &face_source,
            &vertices,
            &face_polygons,
            &brush.faces,
        )
    };
    let chunks = crate::build_brush_chunks(&vertices, &face_polygons, &evaluated_faces);

    let mut fallback_material: Option<Handle<StandardMaterial>> = None;
    for chunk in chunks {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, chunk.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, chunk.normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, chunk.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, chunk.tangents);
        mesh.insert_indices(Indices::U32(chunk.indices));
        let mesh_handle = meshes.add(mesh);

        let material = if chunk.material != Handle::default() {
            chunk.material.clone()
        } else {
            fallback_material
                .get_or_insert_with(|| grid_material(materials, assets))
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

/// The shared grid material applied to faces with no assigned material, built
/// from the embedded grid texture. Cached by the caller's `get_or_insert_with`
/// so it is created at most once per rebuild.
fn grid_material(
    materials: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
) -> Handle<StandardMaterial> {
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
        compute_brush_topology, compute_face_tangent_axes,
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

    /// Whether any chunk mesh of the brush has a vertex on the mirrored (-X)
    /// side. With per-material chunking the whole brush is one mesh, so an
    /// X-mirror is observed through the meshed extent rather than a child count.
    fn has_mirrored_geometry(app: &mut App, brush_entity: Entity) -> bool {
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(brush_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let meshes = app.world().resource::<Assets<Mesh>>();
        children.iter().any(|&child| {
            let Some(mesh3d) = app.world().get::<Mesh3d>(child) else {
                return false;
            };
            let Some(mesh) = meshes.get(&mesh3d.0) else {
                return false;
            };
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .is_some_and(|positions| positions.iter().any(|p| p[0] < -1e-5))
        })
    }

    #[test]
    fn changed_brush_remeshes_on_mutation() {
        let mut app = make_app();

        // A uniform cuboid: all six faces share the default material, so the
        // insert frame meshes it into a single material chunk.
        let brush_entity = app.world_mut().spawn(Brush::cuboid(0.5, 0.5, 0.5)).id();
        app.update();

        let count_after_insert = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count_after_insert, 1,
            "a uniform cuboid meshes into one material chunk on insert"
        );

        // Give one face a distinct material via get_mut. This does NOT re-insert,
        // so Changed<Brush> must detect the mutation and rebuild; the face splits
        // into its own chunk, and the old child must be cleared.
        let distinct = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        {
            let mut brush = app
                .world_mut()
                .get_mut::<Brush>(brush_entity)
                .expect("brush entity exists");
            brush.faces[0].material = distinct;
        }

        app.update();

        let count_after_mutation = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count_after_mutation, 2,
            "a distinct-material face splits into its own chunk; old children cleared"
        );
    }

    #[test]
    fn insert_frame_meshes_exactly_once() {
        let mut app = make_app();

        let brush_entity = app.world_mut().spawn(Brush::cuboid(0.5, 0.5, 0.5)).id();
        app.update();

        // No double-spawn: the clear-then-rebuild pass on the insert frame must
        // not produce two chunks. A uniform cuboid is exactly one material chunk.
        let count = face_mesh_child_count(&mut app, brush_entity);
        assert_eq!(
            count, 1,
            "a uniform cuboid meshes into exactly one chunk, not two"
        );
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
    fn modifier_stack_changes_remesh_the_game_brush() {
        let mut app = make_app();

        // A half-cube (all verts at x >= 0) with a default X-mirror. The mirror
        // adds a -X half, observed through the meshed extent.
        let brush_entity = app
            .world_mut()
            .spawn((half_cube_brush(), mirror_stack()))
            .id();
        app.update();
        assert!(
            has_mirrored_geometry(&mut app, brush_entity),
            "a default X-mirror must mesh a -X half"
        );

        // Changed<ModifierStack> must re-mesh with no brush touch.
        {
            let mut stack = app
                .world_mut()
                .get_mut::<ModifierStack>(brush_entity)
                .expect("stack component exists");
            let Modifier::Mirror(mirror) = &mut stack.modifiers[0].modifier;
            mirror.mirror_x = false;
        }
        app.update();
        assert!(
            !has_mirrored_geometry(&mut app, brush_entity),
            "disabling the mirror axis drops the -X half"
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
        assert!(
            has_mirrored_geometry(&mut app, brush_entity),
            "re-enabling the mirror axis restores the -X half"
        );

        // Removal alone must drop the mirrored half; no brush touch.
        app.world_mut()
            .entity_mut(brush_entity)
            .remove::<ModifierStack>();
        app.update();
        assert!(
            !has_mirrored_geometry(&mut app, brush_entity),
            "removing the modifier stack drops the -X half"
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

        // The authored half-cube sits entirely at x >= 0. If the disabled mirror
        // had been applied, the game mesh would carry mirrored geometry at x < 0,
        // so no chunk vertex may have a negative x.
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(brush_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mut any = false;
        for child in children {
            let Some(mesh3d) = app.world().get::<Mesh3d>(child) else {
                continue;
            };
            any = true;
            let mesh = meshes.get(&mesh3d.0).expect("chunk mesh asset exists");
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .expect("position attribute");
            assert!(
                positions.iter().all(|p| p[0] >= -1e-5),
                "an in_game=false mirror must add no -X geometry"
            );
        }
        assert!(any, "the brush must still mesh its authored faces");
    }

    #[test]
    fn mirrored_cap_face_normals_point_outward() {
        let mut app = make_app();

        let brush_entity = app
            .world_mut()
            .spawn((half_cube_brush(), mirror_stack()))
            .id();
        app.update();

        // All faces share the default material, so they mesh into one chunk.
        // The mirrored copy of the +X cap is the quad whose verts sit at
        // x = -0.5; its face data clones the authored entry, whose un-reflected
        // +X normal would shade and wind it inside out, so the build must
        // recompute the plane from the reflected ring. No vertex on the x = -0.5
        // plane may keep a +X normal, and the recomputed -X cap must be present.
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(brush_entity)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mut found_neg_x_cap = false;
        let mut any = false;
        for child in children {
            let Some(mesh3d) = app.world().get::<Mesh3d>(child) else {
                continue;
            };
            any = true;
            let mesh = meshes.get(&mesh3d.0).expect("chunk mesh asset exists");
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .expect("position attribute");
            let normals = mesh
                .attribute(Mesh::ATTRIBUTE_NORMAL)
                .and_then(|a| a.as_float3())
                .expect("normal attribute");
            for (p, n) in positions.iter().zip(normals.iter()) {
                if (p[0] + 0.5).abs() < 1e-5 {
                    assert!(
                        n[0] <= 1e-5,
                        "no vertex at x = -0.5 may keep the un-reflected +X normal, got {n:?}"
                    );
                    if n[0] < -0.5 {
                        found_neg_x_cap = true;
                    }
                }
            }
        }
        assert!(any, "the mirrored brush must mesh");
        assert!(
            found_neg_x_cap,
            "the recomputed mirrored -X cap (normal -X) must be present"
        );
    }
}
