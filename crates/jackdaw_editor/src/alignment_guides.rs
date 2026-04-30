use bevy::prelude::*;

use crate::brush::BrushMeshCache;
use crate::default_style;
use crate::gizmos::{GizmoDragState, GizmoMode};
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
    gizmo_mode: &GizmoMode,
    modal_state: &ModalTransformState,
    viewport_drag: &ViewportDragState,
) -> bool {
    if gizmo_drag.active && *gizmo_mode == GizmoMode::Translate {
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
    gizmo_mode: &GizmoMode,
    modal_state: &ModalTransformState,
    viewport_drag: &ViewportDragState,
    transforms: &Query<&GlobalTransform>,
) -> Option<(Entity, Vec3)> {
    // Gizmo translate
    if gizmo_drag.active
        && *gizmo_mode == GizmoMode::Translate
        && let Some(e) = gizmo_drag.entity
        && let Ok(gt) = transforms.get(e)
    {
        return Some((e, gt.translation()));
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

/// Cache sorted unique vertex coordinates (per axis) for all non-selected entities at drag start.
fn cache_reference_coords(
    mut state: ResMut<AlignmentGuideState>,
    settings: Res<OverlaySettings>,
    gizmo_drag: Res<GizmoDragState>,
    gizmo_mode: Res<GizmoMode>,
    modal_state: Res<ModalTransformState>,
    viewport_drag: Res<ViewportDragState>,
    non_selected: Query<(Entity, &GlobalTransform, Option<&BrushMeshCache>), Without<Selected>>,
    children_query: Query<&Children>,
    mesh_query: Query<(&Mesh3d, &GlobalTransform)>,
    meshes: Res<Assets<Mesh>>,
) {
    if !settings.show_alignment_guides {
        state.cache_valid = false;
        for coords in &mut state.reference_coords {
            coords.clear();
        }
        return;
    }

    let dragging = is_translate_drag_active(&gizmo_drag, &gizmo_mode, &modal_state, &viewport_drag);

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

    // Build cache
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
            viewport_overlays::collect_descendant_mesh_world_vertices(
                entity,
                &children_query,
                &mesh_query,
                &meshes,
                &mut verts,
            );
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
    gizmo_mode: Res<GizmoMode>,
    modal_state: Res<ModalTransformState>,
    viewport_drag: Res<ViewportDragState>,
    transforms: Query<&GlobalTransform>,
    camera_query: Query<&GlobalTransform, With<crate::viewport::MainViewportCamera>>,
    selected: Query<(Entity, &GlobalTransform, Option<&BrushMeshCache>), With<Selected>>,
    mut selected_transforms: Query<&mut Transform, With<Selected>>,
    children_query: Query<&Children>,
    mesh_query: Query<(&Mesh3d, &GlobalTransform)>,
    meshes: Res<Assets<Mesh>>,
) {
    if !settings.show_alignment_guides {
        return;
    }

    let Some((dragged_entity, drag_pos)) = dragged_entity_position(
        &gizmo_drag,
        &gizmo_mode,
        &modal_state,
        &viewport_drag,
        &transforms,
    ) else {
        return;
    };

    let Ok(cam_tf) = camera_query.single() else {
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
            viewport_overlays::collect_descendant_mesh_world_vertices(
                entity,
                &children_query,
                &mesh_query,
                &meshes,
                &mut dragged_verts,
            );
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
