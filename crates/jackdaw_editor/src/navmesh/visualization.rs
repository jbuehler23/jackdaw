use bevy::{
    asset::RenderAssetUsages,
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::default_style;
use bevy_rerecast::{
    TriMeshFromBevyMesh as _,
    prelude::*,
    rerecast::{DetailNavmesh, PolygonNavmesh, TriMesh},
};

use super::brp_client::{ObstacleGizmo, SceneVisualMesh};
use super::{NavmeshHandleRes, NavmeshState, NavmeshStatus};
use crate::EditorEntity;

/// Marker component for detail fill mesh entities.
#[derive(Component)]
pub struct NavmeshFillMesh;

/// Marker component for the retained-mode gizmo wireframe entity (detail mesh).
#[derive(Component)]
pub struct NavmeshGizmoEntity;

/// Marker component for the polygon mesh wireframe gizmo entity.
#[derive(Component)]
pub struct NavmeshPolyGizmoEntity;

/// Marker component for polygon fill mesh entities.
#[derive(Component)]
pub struct NavmeshPolyFillMesh;

/// Controls visibility of the four navmesh visualization layers.
#[derive(Resource)]
pub struct NavmeshVizConfig {
    pub show_visual: bool,
    pub show_obstacles: bool,
    pub show_detail_mesh: bool,
    pub show_polygon_mesh: bool,
}

impl Default for NavmeshVizConfig {
    fn default() -> Self {
        Self {
            show_visual: true,
            show_obstacles: false,
            show_detail_mesh: true,
            show_polygon_mesh: false,
        }
    }
}

/// Tracks the current navmesh asset ID to detect changes.
#[derive(Resource, Default)]
struct NavmeshVisuals {
    current_id: Option<AssetId<Navmesh>>,
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<NavmeshVisuals>();
    app.init_resource::<NavmeshVizConfig>();
    app.add_systems(
        Update,
        (
            rebuild_navmesh_visuals.run_if(resource_exists::<NavmeshHandleRes>),
            sync_navmesh_viz_visibility,
        )
            .run_if(in_state(crate::AppState::Editor)),
    );
    app.add_observer(on_navmesh_region_removed);
}

fn rebuild_navmesh_visuals(
    mut commands: Commands,
    navmesh_handle: Res<NavmeshHandleRes>,
    navmeshes: Res<Assets<Navmesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut visuals: ResMut<NavmeshVisuals>,
    mut asset_events: MessageReader<AssetEvent<Navmesh>>,
    existing_fills: Query<Entity, With<NavmeshFillMesh>>,
    existing_poly_fills: Query<Entity, (With<NavmeshPolyFillMesh>, Without<NavmeshFillMesh>)>,
    existing_gizmo: Query<(Entity, &Gizmo), With<NavmeshGizmoEntity>>,
    existing_poly_gizmo: Query<
        (Entity, &Gizmo),
        (With<NavmeshPolyGizmoEntity>, Without<NavmeshGizmoEntity>),
    >,
    viz_config: Res<NavmeshVizConfig>,
    mut state: ResMut<NavmeshState>,
) {
    let handle_id = navmesh_handle.id();

    // Check if we need to rebuild
    let handle_changed = visuals.current_id != Some(handle_id);
    let asset_modified = asset_events.read().any(|ev| match ev {
        AssetEvent::Added { id } | AssetEvent::Modified { id } => *id == handle_id,
        _ => false,
    });

    if !handle_changed && !asset_modified {
        return;
    }

    let Some(navmesh) = navmeshes.get(handle_id) else {
        return;
    };

    visuals.current_id = Some(handle_id);

    // --- Despawn old fill meshes ---
    for entity in &existing_fills {
        commands.entity(entity).despawn();
    }
    for entity in &existing_poly_fills {
        commands.entity(entity).despawn();
    }

    let detail = &navmesh.detail;
    let polygon = &navmesh.polygon;

    // --- Build detail fill meshes grouped by area type ---
    let mut area_vertices: std::collections::HashMap<u8, Vec<[f32; 3]>> =
        std::collections::HashMap::new();

    for (submesh_idx, submesh) in detail.meshes.iter().enumerate() {
        let area = if submesh_idx < polygon.areas.len() {
            *polygon.areas[submesh_idx]
        } else {
            0
        };

        let base_v = submesh.base_vertex_index as usize;
        let base_t = submesh.base_triangle_index as usize;
        let verts = &detail.vertices[base_v..base_v + submesh.vertex_count as usize];
        let tris = &detail.triangles[base_t..base_t + submesh.triangle_count as usize];

        let entry = area_vertices.entry(area).or_default();

        for tri in tris {
            for &idx in tri {
                let v = verts[idx as usize];
                entry.push([v.x, v.y, v.z]);
            }
        }
    }

    let detail_fill_vis = if viz_config.show_detail_mesh {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    for (area, vertices) in &area_vertices {
        let color = area_color(*area);
        let vertex_count = vertices.len();

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; vertex_count]);
        let indices: Vec<u32> = (0..vertex_count as u32).collect();
        mesh.insert_indices(Indices::U32(indices));

        let material = materials.add(StandardMaterial {
            base_color: color,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            depth_bias: -10.0,
            ..default()
        });

        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            detail_fill_vis,
            NavmeshFillMesh,
            NotShadowCaster,
            NotShadowReceiver,
            EditorEntity,
        ));
    }

    // --- Build retained-mode gizmo wireframe (detail triangles) ---
    let wireframe_color: Color = default_style::NAVMESH_DETAIL_WIREFRAME;

    if let Ok((_entity, gizmo)) = existing_gizmo.single() {
        if let Some(asset) = gizmo_assets.get_mut(&gizmo.handle) {
            asset.clear();
            if viz_config.show_detail_mesh {
                populate_wireframe(asset, detail, wireframe_color);
            }
        }
    } else {
        let mut gizmo_asset = GizmoAsset::default();
        if viz_config.show_detail_mesh {
            populate_wireframe(&mut gizmo_asset, detail, wireframe_color);
        }

        commands.spawn((
            Gizmo {
                handle: gizmo_assets.add(gizmo_asset),
                line_config: GizmoLineConfig {
                    width: 1.5,
                    perspective: true,
                    ..default()
                },
                depth_bias: -0.003,
            },
            NavmeshGizmoEntity,
            EditorEntity,
        ));
    }

    // --- Build polygon fill mesh ---
    let poly_fill_vis = if viz_config.show_polygon_mesh {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    let poly_fill_verts = build_polygon_fill_vertices(polygon);
    if !poly_fill_verts.is_empty() {
        let vertex_count = poly_fill_verts.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, poly_fill_verts);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; vertex_count]);
        let indices: Vec<u32> = (0..vertex_count as u32).collect();
        mesh.insert_indices(Indices::U32(indices));

        let material = materials.add(StandardMaterial {
            base_color: default_style::NAVMESH_POLYGON_FILL,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            depth_bias: -10.0,
            ..default()
        });

        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            poly_fill_vis,
            NavmeshPolyFillMesh,
            NotShadowCaster,
            NotShadowReceiver,
            EditorEntity,
        ));
    }

    // --- Build polygon mesh wireframe (coarser outlines) ---
    let poly_color: Color = default_style::NAVMESH_POLYGON_WIREFRAME;

    if let Ok((_entity, gizmo)) = existing_poly_gizmo.single() {
        if let Some(asset) = gizmo_assets.get_mut(&gizmo.handle) {
            asset.clear();
            if viz_config.show_polygon_mesh {
                populate_polygon_wireframe(asset, polygon, poly_color);
            }
        }
    } else {
        let mut gizmo_asset = GizmoAsset::default();
        if viz_config.show_polygon_mesh {
            populate_polygon_wireframe(&mut gizmo_asset, polygon, poly_color);
        }

        commands.spawn((
            Gizmo {
                handle: gizmo_assets.add(gizmo_asset),
                line_config: GizmoLineConfig {
                    width: 2.5,
                    perspective: true,
                    ..default()
                },
                depth_bias: -0.004,
            },
            NavmeshPolyGizmoEntity,
            EditorEntity,
        ));
    }

    state.status = NavmeshStatus::Ready;
}

fn on_navmesh_region_removed(
    _trigger: On<Remove, jackdaw_jsn::NavmeshRegion>,
    mut commands: Commands,
    fills: Query<Entity, With<NavmeshFillMesh>>,
    poly_fills: Query<Entity, With<NavmeshPolyFillMesh>>,
    gizmos: Query<Entity, With<NavmeshGizmoEntity>>,
    poly_gizmos: Query<Entity, With<NavmeshPolyGizmoEntity>>,
    scene_visuals: Query<Entity, With<SceneVisualMesh>>,
    obstacle_gizmos: Query<Entity, With<ObstacleGizmo>>,
    mut state: ResMut<NavmeshState>,
) {
    for entity in fills
        .iter()
        .chain(poly_fills.iter())
        .chain(gizmos.iter())
        .chain(poly_gizmos.iter())
        .chain(scene_visuals.iter())
        .chain(obstacle_gizmos.iter())
    {
        commands.entity(entity).despawn();
    }
    commands.queue(|world: &mut World| {
        world.resource_mut::<NavmeshHandleRes>().0 = Default::default();
        world.resource_mut::<NavmeshVisuals>().current_id = None;
    });
    state.status = NavmeshStatus::Idle;
}

fn populate_wireframe(gizmo: &mut GizmoAsset, detail: &DetailNavmesh, color: Color) {
    for submesh in &detail.meshes {
        let base_v = submesh.base_vertex_index as usize;
        let base_t = submesh.base_triangle_index as usize;
        let verts = &detail.vertices[base_v..base_v + submesh.vertex_count as usize];
        let tris = &detail.triangles[base_t..base_t + submesh.triangle_count as usize];

        for tri in tris {
            let a = verts[tri[0] as usize];
            let b = verts[tri[1] as usize];
            let c = verts[tri[2] as usize];
            gizmo.linestrip([a, b, c, a], color);
        }
    }
}

fn populate_polygon_wireframe(gizmo: &mut GizmoAsset, polygon: &PolygonNavmesh, color: Color) {
    let aabb = &polygon.aabb;
    let cs = polygon.cell_size;
    let ch = polygon.cell_height;

    for poly_verts in polygon.polygons() {
        let world_verts: Vec<Vec3> = poly_verts
            .map(|idx| {
                let v = polygon.vertices[idx as usize];
                Vec3::new(
                    aabb.min.x + v.x as f32 * cs,
                    aabb.min.y + v.y as f32 * ch,
                    aabb.min.z + v.z as f32 * cs,
                )
            })
            .collect();

        if world_verts.len() >= 2 {
            let mut strip = world_verts.clone();
            strip.push(world_verts[0]);
            gizmo.linestrip(strip, color);
        }
    }
}

fn build_polygon_fill_vertices(polygon: &PolygonNavmesh) -> Vec<[f32; 3]> {
    let aabb = &polygon.aabb;
    let cs = polygon.cell_size;
    let ch = polygon.cell_height;
    let mut vertices = Vec::new();

    for poly_verts in polygon.polygons() {
        let world_verts: Vec<[f32; 3]> = poly_verts
            .map(|idx| {
                let v = polygon.vertices[idx as usize];
                [
                    aabb.min.x + v.x as f32 * cs,
                    aabb.min.y + v.y as f32 * ch,
                    aabb.min.z + v.z as f32 * cs,
                ]
            })
            .collect();

        // Fan-triangulate: (v0, v1, v2), (v0, v2, v3), ...
        if world_verts.len() >= 3 {
            for i in 1..world_verts.len() - 1 {
                vertices.push(world_verts[0]);
                vertices.push(world_verts[i]);
                vertices.push(world_verts[i + 1]);
            }
        }
    }

    vertices
}

fn sync_navmesh_viz_visibility(
    viz_config: Res<NavmeshVizConfig>,
    navmesh_handle: Option<Res<NavmeshHandleRes>>,
    navmeshes: Res<Assets<Navmesh>>,
    meshes: Res<Assets<Mesh>>,
    mut gizmo_assets: ResMut<Assets<GizmoAsset>>,
    mut scene_visuals: Query<
        &mut Visibility,
        (
            With<SceneVisualMesh>,
            Without<ObstacleGizmo>,
            Without<NavmeshFillMesh>,
            Without<NavmeshPolyFillMesh>,
            Without<NavmeshGizmoEntity>,
            Without<NavmeshPolyGizmoEntity>,
        ),
    >,
    obstacles: Query<(&Mesh3d, &Gizmo), With<ObstacleGizmo>>,
    mut detail_fills: Query<
        &mut Visibility,
        (
            With<NavmeshFillMesh>,
            Without<SceneVisualMesh>,
            Without<ObstacleGizmo>,
            Without<NavmeshPolyFillMesh>,
            Without<NavmeshGizmoEntity>,
            Without<NavmeshPolyGizmoEntity>,
        ),
    >,
    mut poly_fills: Query<
        &mut Visibility,
        (
            With<NavmeshPolyFillMesh>,
            Without<SceneVisualMesh>,
            Without<ObstacleGizmo>,
            Without<NavmeshFillMesh>,
            Without<NavmeshGizmoEntity>,
            Without<NavmeshPolyGizmoEntity>,
        ),
    >,
    detail_gizmos: Query<&Gizmo, (With<NavmeshGizmoEntity>, Without<NavmeshPolyGizmoEntity>)>,
    poly_gizmos: Query<&Gizmo, (With<NavmeshPolyGizmoEntity>, Without<NavmeshGizmoEntity>)>,
) {
    if !viz_config.is_changed() {
        return;
    }

    // Scene visual meshes: visibility toggle.
    let visual_vis = if viz_config.show_visual {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut vis in &mut scene_visuals {
        *vis = visual_vis;
    }

    // Obstacle gizmo: clear/repopulate from mesh data.
    for (mesh3d, gizmo) in &obstacles {
        if let Some(asset) = gizmo_assets.get_mut(&gizmo.handle) {
            asset.clear();
            if viz_config.show_obstacles
                && let Some(mesh) = meshes.get(&mesh3d.0)
                && let Some(trimesh) = TriMesh::from_mesh(mesh)
            {
                populate_obstacle_wireframe(asset, &trimesh);
            }
        }
    }

    // Detail fill meshes: visibility toggle.
    let detail_fill_vis = if viz_config.show_detail_mesh {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut vis in &mut detail_fills {
        *vis = detail_fill_vis;
    }

    // Polygon fill meshes: visibility toggle.
    let poly_fill_vis = if viz_config.show_polygon_mesh {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut vis in &mut poly_fills {
        *vis = poly_fill_vis;
    }

    // Detail gizmo: clear/repopulate.
    for gizmo in &detail_gizmos {
        if let Some(asset) = gizmo_assets.get_mut(&gizmo.handle) {
            asset.clear();
            if viz_config.show_detail_mesh
                && let Some(navmesh) = navmesh_handle.as_ref().and_then(|h| navmeshes.get(h.id()))
            {
                let color: Color = default_style::NAVMESH_DETAIL_WIREFRAME;
                populate_wireframe(asset, &navmesh.detail, color);
            }
        }
    }

    // Polygon gizmo: clear/repopulate.
    for gizmo in &poly_gizmos {
        if let Some(asset) = gizmo_assets.get_mut(&gizmo.handle) {
            asset.clear();
            if viz_config.show_polygon_mesh
                && let Some(navmesh) = navmesh_handle.as_ref().and_then(|h| navmeshes.get(h.id()))
            {
                let color: Color = default_style::NAVMESH_POLYGON_WIREFRAME;
                populate_polygon_wireframe(asset, &navmesh.polygon, color);
            }
        }
    }
}

fn populate_obstacle_wireframe(gizmo: &mut GizmoAsset, trimesh: &TriMesh) {
    let color: Color = default_style::NAVMESH_OBSTACLE_WIREFRAME;
    for indices in &trimesh.indices {
        let verts: Vec<Vec3> = indices
            .to_array()
            .iter()
            .map(|&i| Vec3::from(trimesh.vertices[i as usize]))
            .collect();
        gizmo.linestrip([verts[0], verts[1], verts[2], verts[0]], color);
    }
}

fn area_color(area: u8) -> Color {
    match area {
        0 => default_style::NAVMESH_AREA_0,
        1 => default_style::NAVMESH_AREA_1,
        2 => default_style::NAVMESH_AREA_2,
        3 => default_style::NAVMESH_AREA_3,
        _ => default_style::NAVMESH_AREA_DEFAULT,
    }
}
