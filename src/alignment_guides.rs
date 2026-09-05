use bevy::prelude::*;

use crate::active_tool::ActiveTool;
use crate::brush::BrushMeshCache;
use crate::default_style;
use crate::gizmos::GizmoDragState;
use crate::modal_transform::{ModalOp, ModalTransformState, ViewportDragState};
use crate::selection::Selected;
use crate::viewport_overlays::{self, OverlaySettings};

const ALIGN_THRESHOLD_FACTOR: f32 = 0.005;
const SNAP_THRESHOLD_FACTOR: f32 = 0.003;
/// Epsilon for deduplicating vertex coordinates.
const DEDUP_EPSILON: f32 = 1e-4;

struct AlignCandidate {
    abs_delta: f32,
    delta: f32,
    aligned_val: f32,
}

/// Custom gizmo group for alignment guide lines (thin, depth-biased).
#[derive(Default, Reflect, GizmoConfigGroup)]
struct AlignmentGuideGizmoGroup;

pub struct AlignmentGuidesPlugin;

impl Plugin for AlignmentGuidesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AlignmentGuideState>()
            .init_gizmo_group::<AlignmentGuideGizmoGroup>()
            .add_systems(Startup, configure_alignment_gizmos)
            .add_systems(
                Update,
                (cache_reference_coords, draw_alignment_guides)
                    .chain()
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

fn configure_alignment_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<AlignmentGuideGizmoGroup>();
    config.line.width = 1.0;
    config.depth_bias = -0.5;
}

#[derive(Resource, Default)]
pub struct AlignmentGuideState {
    /// Sorted unique coordinate values from all reference entity vertices, per axis [X, Y, Z].
    pub reference_coords: [Vec<f32>; 3],
    pub cache_valid: bool,
}

/// Returns true if a translation drag is currently active.
fn is_translate_drag_active(
    gizmo_drag: &GizmoDragState,
    active_tool: &ActiveTool,
    modal_state: &ModalTransformState,
    viewport_drag: &ViewportDragState,
) -> bool {
    // Single-target gizmo translate only. For a multi-target group drag,
    // snapping the representative entity alone would drift the group apart,
    // so alignment guides stay off for groups.
    if gizmo_drag.active && *active_tool == ActiveTool::Translate && gizmo_drag.targets.len() == 1 {
        return true;
    }
    if let Some(ref active) = modal_state.active
        && active.op == ModalOp::Grab
    {
        return true;
    }
    viewport_drag.active.is_some()
}

/// Returns the entity being dragged and its current world position.
fn dragged_entity_position(
    gizmo_drag: &GizmoDragState,
    active_tool: &ActiveTool,
    modal_state: &ModalTransformState,
    viewport_drag: &ViewportDragState,
    transforms: &Query<&GlobalTransform>,
) -> Option<(Entity, Vec3)> {
    // Gizmo translate: use the first target entity as the representative.
    if gizmo_drag.active
        && *active_tool == ActiveTool::Translate
        && let Some(t) = gizmo_drag.targets.first()
        && let Ok(gt) = transforms.get(t.entity)
    {
        return Some((t.entity, gt.translation()));
    }
    // Modal grab
    if let Some(ref active) = modal_state.active
        && active.op == ModalOp::Grab
        && let Ok(gt) = transforms.get(active.entity)
    {
        return Some((active.entity, gt.translation()));
    }
    // Viewport drag
    if let Some(ref active) = viewport_drag.active
        && let Ok(gt) = transforms.get(active.entity)
    {
        return Some((active.entity, gt.translation()));
    }
    None
}

/// The queries [`viewport_overlays::collect_measurable_world_vertices`] needs,
/// bundled so both guide systems stay inside bevy's system-argument limit.
#[derive(bevy::ecs::system::SystemParam)]
struct MeasurableGeometry<'w, 's> {
    children_query: Query<'w, 's, &'static Children>,
    mesh_query: Query<'w, 's, (&'static Mesh3d, &'static GlobalTransform)>,
    view_dependent: Query<'w, 's, (), With<crate::ViewDependentBounds>>,
    authored_bounds: Query<'w, 's, &'static bevy::camera::primitives::Aabb>,
    meshes: Res<'w, Assets<Mesh>>,
}

impl MeasurableGeometry<'_, '_> {
    /// See [`viewport_overlays::collect_measurable_world_vertices`].
    fn collect(&self, entity: Entity, global_tf: &GlobalTransform, out: &mut Vec<Vec3>) {
        viewport_overlays::collect_measurable_world_vertices(
            entity,
            &self.children_query,
            &self.mesh_query,
            &self.view_dependent,
            &self.authored_bounds,
            global_tf,
            &self.meshes,
            out,
        );
    }
}

/// Cache sorted unique vertex coordinates (per axis) for all non-selected entities at drag start.
fn cache_reference_coords(
    mut state: ResMut<AlignmentGuideState>,
    settings: Res<OverlaySettings>,
    gizmo_drag: Res<GizmoDragState>,
    active_tool: Res<ActiveTool>,
    modal_state: Res<ModalTransformState>,
    viewport_drag: Res<ViewportDragState>,
    non_selected: Query<(Entity, &GlobalTransform, Option<&BrushMeshCache>), Without<Selected>>,
    geometry: MeasurableGeometry,
) {
    if !settings.show_alignment_guides {
        state.cache_valid = false;
        for coords in &mut state.reference_coords {
            coords.clear();
        }
        return;
    }

    let dragging =
        is_translate_drag_active(&gizmo_drag, &active_tool, &modal_state, &viewport_drag);

    if !dragging {
        state.cache_valid = false;
        for coords in &mut state.reference_coords {
            coords.clear();
        }
        return;
    }

    if state.cache_valid {
        return;
    }

    for coords in &mut state.reference_coords {
        coords.clear();
    }

    for (entity, global_tf, maybe_brush) in &non_selected {
        let world_verts = if let Some(cache) = maybe_brush {
            if cache.vertices.is_empty() {
                continue;
            }
            cache
                .vertices
                .iter()
                .map(|v| global_tf.transform_point(*v))
                .collect::<Vec<Vec3>>()
        } else {
            let mut verts = Vec::new();
            geometry.collect(entity, global_tf, &mut verts);
            if verts.is_empty() {
                continue;
            }
            verts
        };

        for v in &world_verts {
            state.reference_coords[0].push(v.x);
            state.reference_coords[1].push(v.y);
            state.reference_coords[2].push(v.z);
        }
    }

    // Sort and dedup each axis
    for coords in &mut state.reference_coords {
        coords.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        coords.dedup_by(|a, b| (*a - *b).abs() < DEDUP_EPSILON);
    }

    state.cache_valid = true;
}

/// Deduplicate floats within epsilon, returning sorted unique values.
fn dedup_floats(vals: &mut Vec<f32>) {
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    vals.dedup_by(|a, b| (*a - *b).abs() < DEDUP_EPSILON);
}

/// Find the nearest value in a sorted slice to `target` using binary search.
/// Returns `(index, value, abs_delta)` or `None` if the slice is empty.
fn nearest_in_sorted(sorted: &[f32], target: f32) -> Option<(f32, f32)> {
    if sorted.is_empty() {
        return None;
    }
    let idx = sorted.partition_point(|&v| v < target);
    let mut best_val = sorted[0];
    let mut best_delta = (best_val - target).abs();

    if idx < sorted.len() {
        let d = (sorted[idx] - target).abs();
        if d < best_delta {
            best_val = sorted[idx];
            best_delta = d;
        }
    }
    if idx > 0 {
        let d = (sorted[idx - 1] - target).abs();
        if d < best_delta {
            best_val = sorted[idx - 1];
            best_delta = d;
        }
    }
    Some((best_val, best_delta))
}

/// Draw the best-match alignment guide per axis during translation drags.
fn draw_alignment_guides(
    mut gizmos: Gizmos<AlignmentGuideGizmoGroup>,
    state: Res<AlignmentGuideState>,
    settings: Res<OverlaySettings>,
    gizmo_drag: Res<GizmoDragState>,
    active_tool: Res<ActiveTool>,
    modal_state: Res<ModalTransformState>,
    viewport_drag: Res<ViewportDragState>,
    transforms: Query<&GlobalTransform>,
    camera_query: Query<(Entity, &GlobalTransform), With<crate::viewport::MainViewportCamera>>,
    active: Res<crate::viewport::ActiveViewport>,
    selected: Query<(Entity, &GlobalTransform, Option<&BrushMeshCache>), With<Selected>>,
    mut selected_transforms: Query<&mut Transform, With<Selected>>,
    geometry: MeasurableGeometry,
) {
    if !settings.show_alignment_guides {
        return;
    }

    let Some((dragged_entity, drag_pos)) = dragged_entity_position(
        &gizmo_drag,
        &active_tool,
        &modal_state,
        &viewport_drag,
        &transforms,
    ) else {
        return;
    };

    // Multi-viewport: scale guides by the hovered viewport's camera
    // distance, falling back to any camera. Like the gizmo overlay,
    // alignment guides render to all cameras through a single Gizmos
    // pass, so this is correct in the active viewport and approximate
    // in the others.
    let cam_tf = active
        .camera
        .and_then(|e| camera_query.get(e).ok())
        .or_else(|| camera_query.iter().next())
        .map(|(_, tf)| tf);
    let Some(cam_tf) = cam_tf else {
        return;
    };
    let cam_distance = cam_tf.translation().distance(drag_pos);
    let cam_forward = cam_tf.forward().as_vec3();

    // --- Collect dragged entity world-space vertices ---
    let mut dragged_verts = Vec::new();
    for (entity, global_tf, maybe_brush) in &selected {
        if entity != dragged_entity {
            continue;
        }
        if let Some(cache) = maybe_brush {
            for v in &cache.vertices {
                dragged_verts.push(global_tf.transform_point(*v));
            }
        } else {
            geometry.collect(entity, global_tf, &mut dragged_verts);
        }
    }
    if dragged_verts.is_empty() {
        return;
    }

    // Compute dragged entity center for line positioning
    let (d_min, d_max) = viewport_overlays::aabb_from_points(&dragged_verts);
    let d_center = (d_min + d_max) * 0.5;

    // Extract unique coordinate values per axis from dragged vertices
    let mut dragged_coords: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for v in &dragged_verts {
        dragged_coords[0].push(v.x);
        dragged_coords[1].push(v.y);
        dragged_coords[2].push(v.z);
    }
    for coords in &mut dragged_coords {
        dedup_floats(coords);
    }

    // --- Find best alignment candidate per axis ---
    let threshold = cam_distance * ALIGN_THRESHOLD_FACTOR;
    let snap_threshold = cam_distance * SNAP_THRESHOLD_FACTOR;

    let mut best: [Option<AlignCandidate>; 3] = [None, None, None];

    for axis_idx in 0..3 {
        let ref_coords = &state.reference_coords[axis_idx];
        for &d_val in &dragged_coords[axis_idx] {
            if let Some((ref_val, abs_delta)) = nearest_in_sorted(ref_coords, d_val)
                && abs_delta < threshold
            {
                let is_better = match &best[axis_idx] {
                    Some(prev) => abs_delta < prev.abs_delta,
                    None => true,
                };
                if is_better {
                    best[axis_idx] = Some(AlignCandidate {
                        abs_delta,
                        delta: ref_val - d_val,
                        aligned_val: ref_val,
                    });
                }
            }
        }
    }

    // --- Draw viewport-spanning lines + apply snaps ---
    let line_half_extent = cam_distance * 3.0;

    for axis_idx in 0..3 {
        if let Some(candidate) = &best[axis_idx] {
            // Pick the perpendicular axis most orthogonal to the camera (most visible on screen)
            let perp_axes: [(usize, usize); 3] = [(1, 2), (0, 2), (0, 1)];
            let (perp_a, perp_b) = perp_axes[axis_idx];
            let best_perp = if cam_forward[perp_a].abs() < cam_forward[perp_b].abs() {
                perp_a
            } else {
                perp_b
            };
            let other_perp = if best_perp == perp_a { perp_b } else { perp_a };

            let mut start = Vec3::ZERO;
            let mut end = Vec3::ZERO;
            start[axis_idx] = candidate.aligned_val;
            end[axis_idx] = candidate.aligned_val;
            start[other_perp] = d_center[other_perp];
            end[other_perp] = d_center[other_perp];
            start[best_perp] = d_center[best_perp] - line_half_extent;
            end[best_perp] = d_center[best_perp] + line_half_extent;

            gizmos.line(start, end, default_style::ALIGNMENT_GUIDE);

            // Snap
            if candidate.abs_delta < snap_threshold
                && let Ok(mut transform) = selected_transforms.get_mut(dragged_entity)
            {
                match axis_idx {
                    0 => transform.translation.x += candidate.delta,
                    1 => transform.translation.y += candidate.delta,
                    2 => transform.translation.z += candidate.delta,
                    _ => {}
                }
            }
        }
    }
}

/// Alignment guides snap a dragged object to the coordinates of the geometry
/// around it. A terrain draws itself as clipmap rings laid out around the
/// camera, so [`MeasurableGeometry`] supplies its authored extent in place of
/// edges that would move with the viewer.
#[cfg(test)]
mod measurable_geometry_tests {
    use bevy::asset::RenderAssetUsages;
    use bevy::camera::primitives::Aabb;
    use bevy::mesh::PrimitiveTopology;

    use super::*;

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(Assets::<Mesh>::default());
        world
    }

    fn triangle(world: &mut World, offset: Vec3, view_dependent: bool) -> Entity {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                vec![[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0]],
            ),
        );
        let mut entity = world.spawn((
            Mesh3d(mesh),
            Transform::from_translation(offset),
            GlobalTransform::from_translation(offset),
        ));
        if view_dependent {
            entity.insert(crate::ViewDependentBounds);
        }
        entity.id()
    }

    fn collect(world: &mut World, root: Entity) -> Vec<Vec3> {
        world
            .run_system_cached_with(
                |root: In<Entity>,
                 geometry: MeasurableGeometry,
                 transforms: Query<&GlobalTransform>| {
                    let global_tf = transforms.get(*root).copied().unwrap_or_default();
                    let mut out = Vec::new();
                    geometry.collect(*root, &global_tf, &mut out);
                    out
                },
                root,
            )
            .expect("system runs")
    }

    #[test]
    fn a_terrains_authored_edges_are_offered_as_snap_targets() {
        let mut world = world();
        let terrain = world
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                Aabb::from_min_max(Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 3.0, 50.0)),
            ))
            .id();
        let ring = triangle(&mut world, Vec3::new(800.0, 0.0, 800.0), true);
        world.entity_mut(terrain).add_child(ring);

        let verts = collect(&mut world, terrain);

        assert!(!verts.is_empty(), "a terrain has edges worth snapping to");
        let (min, max) = crate::viewport_overlays::aabb_from_points(&verts);
        assert_eq!(min, Vec3::new(-50.0, 0.0, -50.0));
        assert_eq!(max, Vec3::new(50.0, 3.0, 50.0));
    }

    #[test]
    fn ordinary_geometry_is_unaffected() {
        let mut world = world();
        let root = world
            .spawn((Transform::default(), GlobalTransform::default()))
            .id();
        let child = triangle(&mut world, Vec3::new(7.0, 0.0, 0.0), false);
        world.entity_mut(root).add_child(child);

        let verts = collect(&mut world, root);
        assert_eq!(verts.len(), 3);
        let (min, max) = crate::viewport_overlays::aabb_from_points(&verts);
        assert_eq!(min.x, 6.0);
        assert_eq!(max.x, 8.0);
    }

    /// `cache_reference_coords` walks every non-selected entity one at a time, so each
    /// clipmap ring is queried on its own. A ring carries `ViewDependentBounds` and an
    /// `Aabb` bevy re-derives from the viewer-centred lattice, so contributing that `Aabb`
    /// would make a dragged object snap to different coordinates depending on where the
    /// camera stands.
    #[test]
    fn a_ring_offers_no_snap_targets_wherever_the_camera_put_it() {
        let mut world = world();
        let near = triangle(&mut world, Vec3::ZERO, true);
        world
            .entity_mut(near)
            .insert(Aabb::from_min_max(Vec3::splat(-32.0), Vec3::splat(32.0)));
        let far = triangle(&mut world, Vec3::splat(900.0), true);
        world
            .entity_mut(far)
            .insert(Aabb::from_min_max(Vec3::splat(-320.0), Vec3::splat(320.0)));

        assert!(
            collect(&mut world, near).is_empty(),
            "a ring is not a snap target"
        );
        assert_eq!(
            collect(&mut world, near),
            collect(&mut world, far),
            "and moving the camera must not change what is offered"
        );
    }
}
