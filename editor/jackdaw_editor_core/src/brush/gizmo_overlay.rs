use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};

use super::interaction::{
    BrushDragState, EdgeDragState, FaceExtrudeMode, VertexDragConstraint, VertexDragState,
};
use super::{BrushEditMode, BrushMeshCache, BrushSelection, EditMode, LoopCutPreviewLines};
use crate::default_style;
use crate::face_grid::BrushOutlineSelectedGizmoGroup;
use crate::viewport::MainViewportCamera;

/// On-screen radius of a vertex handle, in pixels. The handles are
/// billboarded discs scaled per frame so they hold this size at any zoom.
const VERTEX_HANDLE_PIXELS: f32 = 6.0;

/// Opacity of a handle where it is occluded by geometry, drawn as a depth
/// cue so front and back vertices read apart (edit-mesh x-ray behavior).
const OCCLUDED_HANDLE_ALPHA: f32 = 0.5;

/// Material for the occluded pass of a vertex handle. The depth test is
/// inverted so the disc rasterizes only where it sits behind other
/// geometry, where its base material draws it semi-transparent.
pub(super) type OccludedHandleMaterial = ExtendedMaterial<StandardMaterial, OccludedExtension>;

#[derive(Asset, TypePath, AsBindGroup, Clone, Default)]
pub(super) struct OccludedExtension {}

impl MaterialExtension for OccludedExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
            // Reversed-z: a fragment behind the stored surface has a
            // smaller depth value, so `Less` keeps only occluded pixels.
            depth_stencil.depth_compare = Some(CompareFunction::Less);
            depth_stencil.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// Material for the front (visible) pass of edge ribbons: an unlit standard
/// material with a slope-scaled depth bias so the wireframe clears the
/// coplanar faces it lies on instead of z-fighting them. This is the same
/// rasterizer polygon offset Bevy's wireframe renderer uses.
pub(super) type FrontEdgeMaterial = ExtendedMaterial<StandardMaterial, EdgeOffsetExtension>;

#[derive(Asset, TypePath, AsBindGroup, Clone, Default)]
pub(super) struct EdgeOffsetExtension {}

impl MaterialExtension for EdgeOffsetExtension {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
            depth_stencil.bias.slope_scale = 1.0;
        }
        Ok(())
    }
}

/// Selection state of a handle; indexes the shared per-state materials.
#[derive(Clone, Copy)]
enum HandleState {
    Available = 0,
    Selected = 1,
    Hovered = 2,
}

/// Marks the front (visible) disc of a vertex handle.
#[derive(Component, Default)]
pub(super) struct VertexHandle;

/// Marks the occluded (behind-geometry) disc of a vertex handle.
#[derive(Component, Default)]
pub(super) struct OccludedVertexHandle;

/// Shared render assets: one disc mesh plus a full-opacity material and a
/// dimmed occluded-pass material per selection state.
#[derive(Resource)]
pub(super) struct VertexHandleAssets {
    disc: Handle<Mesh>,
    front: [Handle<StandardMaterial>; 3],
    occluded: [Handle<OccludedHandleMaterial>; 3],
}

pub(super) fn setup_vertex_handle_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut occluded: ResMut<Assets<OccludedHandleMaterial>>,
) {
    let colors = [
        default_style::EDIT_AVAILABLE_COLOR,
        default_style::EDIT_SELECTED_COLOR,
        default_style::EDIT_HOVER_COLOR,
    ];
    let front = colors.map(|color| {
        standard.add(StandardMaterial {
            base_color: color,
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        })
    });
    let occluded = colors.map(|color| {
        occluded.add(OccludedHandleMaterial {
            base: StandardMaterial {
                base_color: color.with_alpha(OCCLUDED_HANDLE_ALPHA),
                unlit: true,
                double_sided: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Blend,
                ..default()
            },
            extension: OccludedExtension::default(),
        })
    });
    commands.insert_resource(VertexHandleAssets {
        // Unit circle in the XY plane (normal +Z); scaled and billboarded
        // toward the camera each frame.
        disc: meshes.add(Circle::new(1.0)),
        front,
        occluded,
    });
}

/// World size of one screen pixel at `dist` from the camera, for either
/// projection. Lets world geometry hold a constant on-screen size.
pub(crate) fn units_per_pixel(projection: &Projection, dist: f32, viewport_height: f32) -> f32 {
    match projection {
        Projection::Perspective(p) => (2.0 * dist * (p.fov * 0.5).tan()) / viewport_height,
        Projection::Orthographic(o) => o.area.height() / viewport_height,
        Projection::Custom(_) => dist * 0.002,
    }
}

/// World radius that renders to a constant pixel size at `dist`.
fn handle_world_radius(projection: &Projection, dist: f32, viewport_height: f32) -> f32 {
    VERTEX_HANDLE_PIXELS * units_per_pixel(projection, dist, viewport_height)
}

/// Maintain two pools of billboarded disc meshes, one per vertex of every
/// edit brush: a full-opacity front pass for visible vertices and a dimmed
/// pass for the ones behind geometry. Picking stays the cache-based
/// screen-space path; this only changes how the handles are drawn.
pub(super) fn update_vertex_handles(
    edit_mode: Res<EditMode>,
    brush_selection: Res<BrushSelection>,
    hover: Res<super::BrushFaceHover>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    camera: Query<(&GlobalTransform, &Projection, &Camera), With<MainViewportCamera>>,
    assets: Option<Res<VertexHandleAssets>>,
    mut front_handles: Query<
        (
            Entity,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (With<VertexHandle>, Without<OccludedVertexHandle>),
    >,
    mut occluded_handles: Query<
        (
            Entity,
            &mut Transform,
            &mut MeshMaterial3d<OccludedHandleMaterial>,
        ),
        (With<OccludedVertexHandle>, Without<VertexHandle>),
    >,
    mut commands: Commands,
) {
    let Some(assets) = assets else {
        return;
    };

    // Gather the world position and selection state for every vertex to show.
    let mut desired: Vec<(Vec3, HandleState)> = Vec::new();
    if let EditMode::BrushEdit(mode) = *edit_mode
        && matches!(mode, BrushEditMode::Vertex | BrushEditMode::Knife)
    {
        for brush_entity in brush_selection.edit_brushes() {
            let (Ok(cache), Ok(brush_global)) = (
                brush_caches.get(brush_entity),
                brush_transforms.get(brush_entity),
            ) else {
                continue;
            };
            let sub = brush_selection.sub(brush_entity);
            let hover_vi = if hover.entity == Some(brush_entity) {
                hover.vertex_index
            } else {
                None
            };
            for (vi, v) in cache.vertices.iter().enumerate() {
                // Bisect-introduced cut geometry has no authored origin and
                // draws no editable handle.
                if cache.authored_vert(vi).is_none() {
                    continue;
                }
                let selected = sub.is_some_and(|s| s.vertices.contains(&vi));
                let state = if hover_vi == Some(vi) && !selected {
                    HandleState::Hovered
                } else if selected {
                    HandleState::Selected
                } else {
                    HandleState::Available
                };
                desired.push((brush_global.transform_point(*v), state));
            }
        }
    }

    // Without a viewport camera there is nothing to billboard against; clear.
    let Ok((cam_global, projection, cam)) = camera.single() else {
        let stale: Vec<Entity> = front_handles
            .iter()
            .map(|(e, ..)| e)
            .chain(occluded_handles.iter().map(|(e, ..)| e))
            .collect();
        for entity in stale {
            commands.entity(entity).despawn();
        }
        return;
    };
    let cam_pos = cam_global.translation();
    let viewport_height = cam.logical_viewport_size().map_or(1080.0, |s| s.y);

    let billboards: Vec<BillboardHandle> = desired
        .iter()
        .map(|(world_pos, state)| {
            let dist = (cam_pos - *world_pos).length().max(1e-4);
            let si = *state as usize;
            BillboardHandle {
                world: *world_pos,
                radius: handle_world_radius(projection, dist, viewport_height),
                front: assets.front[si].clone(),
                occluded: assets.occluded[si].clone(),
            }
        })
        .collect();

    reconcile_billboard_pools::<VertexHandle, OccludedVertexHandle, _, _>(
        &mut commands,
        &assets.disc,
        cam_pos,
        &billboards,
        &mut front_handles,
        &mut occluded_handles,
    );
}

/// One billboarded handle to reconcile: its world position, billboard radius,
/// and the resolved front / occluded material handles.
pub(super) struct BillboardHandle {
    pub world: Vec3,
    pub radius: f32,
    pub front: Handle<StandardMaterial>,
    pub occluded: Handle<OccludedHandleMaterial>,
}

/// Reconcile two pools of billboarded disc handles against `desired`: a
/// depth-tested front pass nudged toward the camera, and an inverted-depth
/// occluded pass on the point. Updates the i-th existing handle in place,
/// spawns when a pool is short, and despawns the surplus. `FrontMarker` /
/// `OccludedMarker` tag the spawned entities so the caller's queries find them.
pub(super) fn reconcile_billboard_pools<FrontMarker, OccludedMarker, FrontFilter, OccludedFilter>(
    commands: &mut Commands,
    disc: &Handle<Mesh>,
    cam_pos: Vec3,
    desired: &[BillboardHandle],
    front_handles: &mut Query<
        (
            Entity,
            &mut Transform,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        FrontFilter,
    >,
    occluded_handles: &mut Query<
        (
            Entity,
            &mut Transform,
            &mut MeshMaterial3d<OccludedHandleMaterial>,
        ),
        OccludedFilter,
    >,
) where
    FrontMarker: Component + Default,
    OccludedMarker: Component + Default,
    FrontFilter: bevy::ecs::query::QueryFilter,
    OccludedFilter: bevy::ecs::query::QueryFilter,
{
    let front_existing: Vec<Entity> = front_handles.iter().map(|(e, ..)| e).collect();
    let occluded_existing: Vec<Entity> = occluded_handles.iter().map(|(e, ..)| e).collect();

    for (i, handle) in desired.iter().enumerate() {
        let to_cam = cam_pos - handle.world;
        let dist = to_cam.length().max(1e-4);
        let dir = to_cam / dist;
        let rotation = Quat::from_rotation_arc(Vec3::Z, dir);

        // Front pass: nudged toward the camera so a handle on a face wins the
        // depth test instead of z-fighting it.
        let front_t = Transform {
            translation: handle.world + dir * handle.radius * 0.5,
            rotation,
            scale: Vec3::splat(handle.radius),
        };
        if let Some(&entity) = front_existing.get(i) {
            if let Ok((_, mut t, mut m)) = front_handles.get_mut(entity) {
                *t = front_t;
                if m.0 != handle.front {
                    m.0 = handle.front.clone();
                }
            }
        } else {
            commands.spawn((
                FrontMarker::default(),
                crate::EditorEntity,
                Mesh3d(disc.clone()),
                MeshMaterial3d::<StandardMaterial>(handle.front.clone()),
                front_t,
            ));
        }

        // Occluded pass: sits on the point; its inverted depth test only
        // rasterizes where the disc is behind other geometry.
        let occ_t = Transform {
            translation: handle.world,
            rotation,
            scale: Vec3::splat(handle.radius),
        };
        if let Some(&entity) = occluded_existing.get(i) {
            if let Ok((_, mut t, mut m)) = occluded_handles.get_mut(entity) {
                *t = occ_t;
                if m.0 != handle.occluded {
                    m.0 = handle.occluded.clone();
                }
            }
        } else {
            commands.spawn((
                OccludedMarker::default(),
                crate::EditorEntity,
                Mesh3d(disc.clone()),
                MeshMaterial3d::<OccludedHandleMaterial>(handle.occluded.clone()),
                occ_t,
            ));
        }
    }

    for &entity in front_existing.iter().skip(desired.len()) {
        commands.entity(entity).despawn();
    }
    for &entity in occluded_existing.iter().skip(desired.len()) {
        commands.entity(entity).despawn();
    }
}

/// On-screen width of an edit-mode edge ribbon, in pixels.
const EDGE_WIDTH_PIXELS: f32 = 1.5;

/// Marks the visible (front) edge ribbon meshes (one per selection state).
#[derive(Component)]
pub(super) struct FrontEdgeMesh;

/// Marks the occluded (behind-geometry) edge ribbon meshes.
#[derive(Component)]
pub(super) struct OccludedEdgeMesh;

/// Handles to the per-state edge ribbon meshes (indexed by `HandleState`),
/// rewritten every frame. Color comes from each mesh's material, so no
/// vertex colors are needed.
#[derive(Resource)]
pub(super) struct EdgeOverlayAssets {
    front: [Handle<Mesh>; 3],
    occluded: [Handle<Mesh>; 3],
    front_entities: [Entity; 3],
    occluded_entities: [Entity; 3],
}

/// Per-state ribbon geometry accumulated for one frame.
#[derive(Default)]
struct EdgeRibbons {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

fn placeholder_edge_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
    mesh
}

fn write_edge_mesh(mesh: &mut Mesh, ribbons: &EdgeRibbons) {
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, ribbons.positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, ribbons.normals.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, ribbons.uvs.clone());
    mesh.insert_indices(Indices::U32(ribbons.indices.clone()));
}

pub(super) fn setup_edge_overlay(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut front_materials: ResMut<Assets<FrontEdgeMaterial>>,
    mut occluded: ResMut<Assets<OccludedHandleMaterial>>,
    existing: Query<Entity, Or<(With<FrontEdgeMesh>, With<OccludedEdgeMesh>)>>,
) {
    // Re-entering the editor must not leave a second set of meshes behind.
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let colors = [
        default_style::EDIT_AVAILABLE_COLOR,
        default_style::EDIT_SELECTED_COLOR,
        default_style::EDIT_HOVER_COLOR,
    ];
    let front: [Handle<Mesh>; 3] = std::array::from_fn(|_| meshes.add(placeholder_edge_mesh()));
    let occluded_meshes: [Handle<Mesh>; 3] =
        std::array::from_fn(|_| meshes.add(placeholder_edge_mesh()));
    let mut front_entities = [Entity::PLACEHOLDER; 3];
    let mut occluded_entities = [Entity::PLACEHOLDER; 3];
    for (i, color) in colors.iter().enumerate() {
        let front_mat = front_materials.add(FrontEdgeMaterial {
            base: StandardMaterial {
                base_color: *color,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            },
            extension: EdgeOffsetExtension::default(),
        });
        front_entities[i] = commands
            .spawn((
                FrontEdgeMesh,
                crate::EditorEntity,
                Mesh3d(front[i].clone()),
                MeshMaterial3d(front_mat),
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        let occluded_mat = occluded.add(OccludedHandleMaterial {
            base: StandardMaterial {
                base_color: color.with_alpha(OCCLUDED_HANDLE_ALPHA),
                unlit: true,
                double_sided: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Blend,
                ..default()
            },
            extension: OccludedExtension::default(),
        });
        occluded_entities[i] = commands
            .spawn((
                OccludedEdgeMesh,
                crate::EditorEntity,
                Mesh3d(occluded_meshes[i].clone()),
                MeshMaterial3d(occluded_mat),
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
    }
    commands.insert_resource(EdgeOverlayAssets {
        front,
        occluded: occluded_meshes,
        front_entities,
        occluded_entities,
    });
}

/// Rebuild the edit-mode edge wireframe as camera-facing ribbon meshes,
/// grouped by selection state: a full-opacity front mesh and a dimmed
/// occluded mesh (depth test inverted) per state, so edges hold a constant
/// on-screen width and read through occluding geometry like vertex handles.
pub(super) fn update_edge_overlay(
    mut commands: Commands,
    edit_mode: Res<EditMode>,
    brush_selection: Res<BrushSelection>,
    hover: Res<super::BrushFaceHover>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    camera: Query<(&GlobalTransform, &Projection, &Camera), With<MainViewportCamera>>,
    assets: Option<Res<EdgeOverlayAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(assets) = assets else {
        return;
    };

    let mut by_state: [EdgeRibbons; 3] = Default::default();

    // The edit-mesh edges are the wireframe in every sub-mode (the object
    // wireframe stands down while editing). Edges carry selection / hover
    // colors only where edges are the selectable element; in vertex / face
    // mode they are the resting wireframe. Clip mode hides the wireframe.
    let editing = matches!(
        *edit_mode,
        EditMode::BrushEdit(
            BrushEditMode::Vertex
                | BrushEditMode::Edge
                | BrushEditMode::Face
                | BrushEditMode::Knife
        )
    );
    let edges_selectable = matches!(
        *edit_mode,
        EditMode::BrushEdit(BrushEditMode::Edge | BrushEditMode::Knife)
    );
    if editing && let Ok((cam_global, projection, cam)) = camera.single() {
        let cam_pos = cam_global.translation();
        let viewport_height = cam.logical_viewport_size().map_or(1080.0, |s| s.y);

        for brush_entity in brush_selection.edit_brushes() {
            let (Ok(cache), Ok(brush_global)) = (
                brush_caches.get(brush_entity),
                brush_transforms.get(brush_entity),
            ) else {
                continue;
            };
            let sub = brush_selection.sub(brush_entity);
            let hover_edge = if edges_selectable && hover.entity == Some(brush_entity) {
                hover.edge
            } else {
                None
            };
            for (a, b) in cache.unique_edges() {
                // A cut/cap edge (either endpoint is bisect geometry) has no
                // authored origin and draws no selectable edit edge.
                if cache.authored_edge((a, b)).is_none() {
                    continue;
                }
                let selected = edges_selectable && sub.is_some_and(|s| s.edges.contains(&(a, b)));
                let state =
                    if hover_edge.is_some_and(|he| he == (a, b) || he == (b, a)) && !selected {
                        HandleState::Hovered
                    } else if selected {
                        HandleState::Selected
                    } else {
                        HandleState::Available
                    };
                let wa = brush_global.transform_point(cache.vertices[a]);
                let wb = brush_global.transform_point(cache.vertices[b]);
                let edge = wb - wa;
                if edge.length_squared() < 1e-12 {
                    continue;
                }
                let edge_dir = edge.normalize();
                let to_a = cam_pos - wa;
                let to_b = cam_pos - wb;
                let dist_a = to_a.length().max(1e-4);
                let dist_b = to_b.length().max(1e-4);
                let dir_a = to_a / dist_a;
                let dir_b = to_b / dist_b;
                let upp_a = units_per_pixel(projection, dist_a, viewport_height);
                let upp_b = units_per_pixel(projection, dist_b, viewport_height);
                let perp_a =
                    edge_dir.cross(dir_a).normalize_or_zero() * (0.5 * EDGE_WIDTH_PIXELS * upp_a);
                let perp_b =
                    edge_dir.cross(dir_b).normalize_or_zero() * (0.5 * EDGE_WIDTH_PIXELS * upp_b);
                // The front material's slope-scaled depth bias clears the face
                // the edge lies on, so the ribbon sits exactly on the edge.
                let ribbons = &mut by_state[state as usize];
                let base = ribbons.positions.len() as u32;
                let quad = [wa + perp_a, wa - perp_a, wb - perp_b, wb + perp_b];
                for v in quad {
                    ribbons.positions.push(v.to_array());
                    ribbons.normals.push([0.0, 0.0, 1.0]);
                    ribbons.uvs.push([0.0, 0.0]);
                }
                ribbons.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
            }
        }
    }

    for (i, ribbons) in by_state.iter().enumerate() {
        let visible = !ribbons.positions.is_empty();
        let visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        commands.entity(assets.front_entities[i]).insert(visibility);
        commands
            .entity(assets.occluded_entities[i])
            .insert(visibility);
        if !visible {
            continue;
        }
        if let Some(mut mesh) = meshes.get_mut(&assets.front[i]) {
            write_edge_mesh(&mut mesh, ribbons);
        }
        if let Some(mut mesh) = meshes.get_mut(&assets.occluded[i]) {
            write_edge_mesh(&mut mesh, ribbons);
        }
    }
}

/// Draw the outline of `face_index` from `cache`, bounds-checking both the
/// face index and every vertex index. A destructive edit (e.g. face delete)
/// shrinks the topology while a stale hover or selection index lingers for a
/// frame, so indexing has to tolerate an out-of-range value instead of
/// panicking.
fn draw_face_outline(
    gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>,
    brush_global: &GlobalTransform,
    cache: &BrushMeshCache,
    face_index: usize,
    color: Color,
) {
    let Some(polygon) = cache.face_polygons.get(face_index) else {
        return;
    };
    if polygon.len() < 3 {
        return;
    }
    for i in 0..polygon.len() {
        let (Some(a), Some(b)) = (
            cache.vertices.get(polygon[i]),
            cache.vertices.get(polygon[(i + 1) % polygon.len()]),
        ) else {
            continue;
        };
        gizmos.line(
            brush_global.transform_point(*a),
            brush_global.transform_point(*b),
            color,
        );
    }
}

pub(super) fn draw_brush_edit_gizmos(
    edit_mode: Res<EditMode>,
    brush_selection: Res<BrushSelection>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    vertex_drag: Res<VertexDragState>,
    edge_drag: Res<EdgeDragState>,
    face_drag: Res<BrushDragState>,
    hover: Res<super::BrushFaceHover>,
    mut gizmos: Gizmos<BrushOutlineSelectedGizmoGroup>,
) {
    // Draw hover face outline (works in both Object and Edit modes).
    // In edit mode the hover entity may be any edit brush, not just the active one.
    if let (Some(hover_entity), Some(hover_face)) = (hover.entity, hover.face_index)
        && let Ok(cache) = brush_caches.get(hover_entity)
        && let Ok(brush_global) = brush_transforms.get(hover_entity)
    {
        // Skip if face is already selected (avoid double highlight).
        let is_selected = brush_selection
            .sub(hover_entity)
            .is_some_and(|s| s.faces.contains(&hover_face));
        if !is_selected {
            draw_face_outline(
                &mut gizmos,
                brush_global,
                cache,
                hover_face,
                default_style::EDIT_HOVER_COLOR,
            );
        }
    }

    let EditMode::BrushEdit(mode) = *edit_mode else {
        return;
    };

    // Collect edit brushes to avoid holding an immutable borrow on
    // brush_selection while we call sub() below.
    let edit_brushes: Vec<Entity> = brush_selection.edit_brushes().collect();
    let active_brush = brush_selection.active_brush;

    if edit_brushes.is_empty() {
        return;
    }

    // Draw handles on every edit brush. All selected brushes are equally
    // editable, so their handles share one resting color.
    for &brush_entity in &edit_brushes {
        let Ok(cache) = brush_caches.get(brush_entity) else {
            continue;
        };
        let Ok(brush_global) = brush_transforms.get(brush_entity) else {
            continue;
        };

        let sub = brush_selection.sub(brush_entity);

        // Vertex handles and edge wireframes are drawn as meshes by
        // `update_vertex_handles` and `update_edge_overlay`, not as
        // immediate-mode gizmos.

        // Highlight selected faces.
        if mode == BrushEditMode::Face {
            let faces = sub.map(|s| s.faces.as_slice()).unwrap_or(&[]);
            for &face_idx in faces {
                draw_face_outline(
                    &mut gizmos,
                    brush_global,
                    cache,
                    face_idx,
                    default_style::EDIT_SELECTED_COLOR,
                );
            }
        }
    }

    // Drag constraint line and extend preview use the active brush's transform.
    // If there is no active brush, skip these overlays.
    let Some(active_entity) = active_brush else {
        return;
    };
    let Ok(active_global) = brush_transforms.get(active_entity) else {
        return;
    };

    // Draw extend mode wireframe preview
    if face_drag.active && face_drag.extrude_mode == FaceExtrudeMode::Extend {
        let polygon = &face_drag.extend_face_polygon;
        let depth = face_drag.extend_depth;
        let normal = face_drag.extend_face_normal;
        let offset = normal * depth;
        let preview_color = default_style::FACE_EXTRUDE_PREVIEW;

        if polygon.len() >= 3 {
            // Base polygon edges
            for i in 0..polygon.len() {
                let a = polygon[i];
                let b = polygon[(i + 1) % polygon.len()];
                gizmos.line(a, b, preview_color);
            }
            // Top polygon edges (base + offset)
            for i in 0..polygon.len() {
                let a = polygon[i] + offset;
                let b = polygon[(i + 1) % polygon.len()] + offset;
                gizmos.line(a, b, preview_color);
            }
            // Connecting edges
            for &v in polygon {
                gizmos.line(v, v + offset, preview_color);
            }
        }
    }

    // Draw drag constraint line (vertex or edge drag)
    let active_constraint = if vertex_drag.active {
        Some(vertex_drag.constraint)
    } else if edge_drag.active {
        Some(edge_drag.constraint)
    } else {
        None
    };
    if let Some(constraint) = active_constraint
        && constraint != VertexDragConstraint::Free
    {
        let (axis_dir, color) = match constraint {
            VertexDragConstraint::AxisX => (Vec3::X, default_style::AXIS_X),
            VertexDragConstraint::AxisY => (Vec3::Y, default_style::AXIS_Y),
            VertexDragConstraint::AxisZ => (Vec3::Z, default_style::AXIS_Z),
            VertexDragConstraint::Free => unreachable!(),
        };
        let (_, brush_rot, _) = active_global.to_scale_rotation_translation();
        let world_axis = brush_rot * axis_dir;
        let center = active_global.translation();
        gizmos.line(
            center - world_axis * 50.0,
            center + world_axis * 50.0,
            color,
        );
    }
}

/// Draw cyan line segments for the loop cut preview, sourced from `LoopCutPreviewLines`.
pub(super) fn draw_loop_cut_preview(
    preview_lines: Res<LoopCutPreviewLines>,
    mut gizmos: Gizmos<BrushOutlineSelectedGizmoGroup>,
) {
    for &(a, b) in &preview_lines.lines {
        gizmos.line(a, b, Color::srgb(0.3, 0.85, 1.0));
    }
}
