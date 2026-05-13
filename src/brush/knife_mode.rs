//! Knife edit mode: incremental, multi-segment face bisection.
//!
//! Mirrors the `BrushEditMode::Clip` pattern. Pressing `K` with a brush
//! selected enters Knife mode; pressing `K` again or Escape exits and
//! discards any in-progress path.
//!
//! Each frame, the cursor snaps to the closest snap target within a
//! small tolerance. Priority order (highest first):
//!
//! - **Path-point snap**: any point already placed in the current path
//!   (large hollow red circle). Lets the user close loops.
//! - **Vertex snap**: filled red dot.
//! - **Edge midpoint snap** (Shift held): hollow red square.
//! - **Edge point snap**: small red diamond on the edge.
//! - **Face interior snap**: small filled red square. The cursor is
//!   over a face but not near any vert or edge; commit will `face_poke`
//!   a center vert at this position before bisecting.
//!
//! LMB click commits the current snap target into the path. Enter
//! bisects each adjacent pair of points as a single `SetBrush` undo
//! entry. Esc or RMB cancels the in-progress path.
//!
//! Commit handling per segment:
//!
//! - Same face: `split_edge` each endpoint that landed on an edge,
//!   `face_poke` each endpoint that landed in the face interior, then
//!   `split_face` connects the two resulting verts with a chord.
//! - Adjacent faces (one shared edge): split the shared edge at the
//!   3D segment-edge intersection, bisect each face with the
//!   intermediate vert.
//! - Non-adjacent faces or faces sharing more than one edge: warn and
//!   skip.
//!
//! Cut-through (Blender-style: project the cut through the whole brush
//! so the back face is cut simultaneously) is filed as task #97.
//!
//! Known limitation: if two consecutive path points both snap to
//! interior points of the *same* original edge, the second segment's
//! lookup will fail (the original edge no longer exists after the
//! first segment's `split_edge`) and the segment is skipped. Most
//! useful paths don't hit this case.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use jackdaw_geometry::editmesh::ops::edge_split::split_edge;
use jackdaw_geometry::editmesh::ops::face_poke::face_poke;
use jackdaw_geometry::editmesh::ops::face_split::split_face;
use jackdaw_geometry::editmesh::{EdgeKey, EditMesh, FaceKey, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{
    BrushEditMesh, BrushEditMode, BrushMeshCache, BrushSelection, EditMode, SetBrush,
};
use crate::commands::CommandHistory;
use crate::default_style;
use crate::face_grid::BrushOutlineSelectedGizmoGroup;
use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};
use crate::viewport_util::{ViewportRemap, point_in_polygon_2d};

/// Screen-space pixel tolerance for snapping the cursor onto a vert, an
/// edge midpoint, or an edge interior. Specified in window logical
/// pixels; converted to camera-target pixels per frame so HiDPI and
/// fractional UI scaling don't shrink the snap region.
const KNIFE_SNAP_PIXELS: f32 = 12.0;

/// Red used for every knife visual (matches the draw-brush cut-mode
/// color in `default_style`).
const KNIFE_COLOR: Color = Color::srgb(1.0, 0.2, 0.2);

/// Kind of snap target chosen for the current cursor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnifeSnapKind {
    /// Snapped to an existing vert within tolerance.
    Vertex,
    /// Snapped to an edge midpoint (Shift held + within edge tolerance).
    EdgeMidpoint,
    /// Snapped to the closest point on an edge.
    EdgePoint,
    /// Cursor is inside a face but not near any vert or edge. Commit
    /// will `face_poke` a center vert at this position.
    FaceInterior,
    /// Snapped to a point already placed in the current path. The
    /// referenced path-point index lets the commit reuse that point's
    /// resolved vert without creating duplicate geometry.
    PathPoint,
}

/// A snap target captured at a single frame's cursor position. Also used
/// as a path-point payload once an LMB click commits one.
#[derive(Clone, Debug)]
pub struct KnifeSnapTarget {
    /// World-space snap point (for preview).
    pub world_pos: Vec3,
    /// Brush-local snap point (for stable mesh ops).
    pub local_pos: Vec3,
    pub kind: KnifeSnapKind,
    /// Which face of the brush the snap was computed against.
    pub face_idx: usize,
    /// Canonical (min, max) vert indices of the edge the snap is on, or
    /// `None` if the snap landed on an existing vert.
    pub edge_pair: Option<(usize, usize)>,
    /// Existing vert index if the snap reused a corner.
    pub vert_idx: Option<usize>,
    /// When `kind == PathPoint`, the index into the live `path` that
    /// the cursor snapped to. `None` for every other kind.
    pub path_point_idx: Option<usize>,
}

/// A point already placed by an LMB click. Holds enough information to
/// resolve the corresponding `VertKey` in a freshly-lifted `EditMesh`
/// at commit time (edge-splitting, face-poking, or vert-reuse).
#[derive(Clone, Debug)]
pub struct KnifePathPoint {
    pub world_pos: Vec3,
    pub local_pos: Vec3,
    pub face_idx: usize,
    pub kind: KnifeSnapKind,
    pub edge_pair: Option<(usize, usize)>,
    pub vert_idx: Option<usize>,
    /// When this entry was a `PathPoint` snap (re-click on an existing
    /// path point), the index of the original point it references.
    /// At commit time the resolved `VertKey` of the original point is
    /// reused, preventing duplicate geometry at the same position.
    pub source_path_idx: Option<usize>,
}

impl From<&KnifeSnapTarget> for KnifePathPoint {
    fn from(s: &KnifeSnapTarget) -> Self {
        Self {
            world_pos: s.world_pos,
            local_pos: s.local_pos,
            face_idx: s.face_idx,
            kind: s.kind,
            edge_pair: s.edge_pair,
            vert_idx: s.vert_idx,
            source_path_idx: s.path_point_idx,
        }
    }
}

/// Knife edit-mode state. Mirrors the `ClipState` shape: cleared
/// whenever the user leaves the mode or commits the cuts.
#[derive(Resource, Default)]
pub struct KnifeMode {
    /// The brush this knife path is being built on. Cleared when the
    /// mode is exited or the active brush changes.
    pub brush_entity: Option<Entity>,
    /// Points clicked so far. Each adjacent pair becomes one (or two)
    /// face bisections at commit time.
    pub path: Vec<KnifePathPoint>,
    /// Snap target under the cursor this frame. Refreshed every frame
    /// by `handle_knife_mode`. `None` when off-brush, off-face, or
    /// outside the snap tolerance.
    pub hover_snap: Option<KnifeSnapTarget>,
}

impl KnifeMode {
    pub fn clear(&mut self) {
        self.brush_entity = None;
        self.path.clear();
        self.hover_snap = None;
    }
}

/// Per-frame cursor handling: refresh `KnifeMode.hover_snap` from the
/// current cursor position, handle LMB (place point), RMB (cancel),
/// and Enter (commit).
///
/// Lives alongside `handle_clip_mode` in the brush plugin's update
/// schedule. Mouse-button gestures aren't expressible as BEI key
/// bindings, so they live inline here rather than as separate
/// operators.
pub(super) fn handle_knife_mode(
    edit_mode: Res<EditMode>,
    active: Res<ActiveViewport>,
    selection: Res<BrushSelection>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
    mut knife: ResMut<KnifeMode>,
    keybind_focus: crate::keybind_focus::KeybindFocus,
) {
    // Bail (and clear any stale state) when the user is not in Knife mode.
    if !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Knife)) {
        if knife.brush_entity.is_some() || !knife.path.is_empty() || knife.hover_snap.is_some() {
            knife.clear();
        }
        return;
    }

    // While the user is typing, ignore all knife input. The hover
    // indicator still updates so the preview matches the cursor.
    let typing = keybind_focus.is_typing();

    // Resolve the active brush; bail (without clearing the path) if
    // the selection is gone.
    let Some(brush_entity) = selection.entity else {
        return;
    };
    // If the active brush changed under us (e.g. user selected a
    // different brush via the outliner while in Knife mode), discard
    // the in-progress path so we don't try to bisect the wrong mesh.
    if let Some(existing) = knife.brush_entity
        && existing != brush_entity
    {
        knife.path.clear();
    }
    knife.brush_entity = Some(brush_entity);

    // Cursor / camera / viewport plumbing. We use the hovered viewport
    // (via `ActiveViewport`) so knife mode works in multi-viewport
    // layouts the same way clip mode does.
    let Ok(window) = primary_window.single() else {
        knife.hover_snap = None;
        return;
    };
    let Some(cursor_pos) = window.cursor_position() else {
        knife.hover_snap = None;
        return;
    };
    let Some(camera_entity) = active.camera else {
        knife.hover_snap = None;
        return;
    };
    let Some(viewport_entity) = active.ui_node else {
        knife.hover_snap = None;
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.get(camera_entity) else {
        knife.hover_snap = None;
        return;
    };
    let Some(viewport_cursor) =
        cursor_in_viewport(cursor_pos, camera, viewport_entity, &viewport_query)
    else {
        knife.hover_snap = None;
        return;
    };

    let Ok(cache) = brush_caches.get(brush_entity) else {
        knife.hover_snap = None;
        return;
    };
    let Ok(brush_global) = brush_transforms.get(brush_entity) else {
        knife.hover_snap = None;
        return;
    };

    // Resolve snap. Path-point snap is the highest priority; it
    // overrides face / vert / edge snap so the user can close loops
    // even when an existing path point sits on top of a vert.
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let path_snap = compute_path_point_snap(
        viewport_cursor,
        &knife.path,
        camera,
        cam_tf,
        viewport_entity,
        &viewport_query,
    );

    let hover_face = pick_face_under_cursor(viewport_cursor, cache, brush_global, camera, cam_tf);
    let geometry_snap = hover_face.and_then(|face_idx| {
        compute_face_snap(
            face_idx,
            viewport_cursor,
            cache,
            brush_global,
            camera,
            cam_tf,
            viewport_entity,
            &viewport_query,
            shift,
        )
    });

    // Fall back to face-interior snap when nothing else stuck but the
    // cursor is over a face. We project a ray through the cursor onto
    // the face plane to get a stable world-space point.
    let interior_snap = if geometry_snap.is_none()
        && let Some(face_idx) = hover_face
    {
        compute_face_interior_snap(
            face_idx,
            viewport_cursor,
            cache,
            brush_global,
            camera,
            cam_tf,
        )
    } else {
        None
    };

    knife.hover_snap = path_snap.or(geometry_snap).or(interior_snap);

    if typing {
        return;
    }

    // Cancel: Escape or RMB. Escape *just* clears the path (leaving
    // the mode active so the user can keep cutting); the Knife-mode
    // toggle operator handles the global "exit mode" Escape via the
    // `can_exit_brush_edit` gate in `edit_mode_ops`.
    if mouse.just_pressed(MouseButton::Right) {
        knife.path.clear();
        return;
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        // Clearing the path stops the "in progress" state, but leaves
        // the mode active. The brush-exit handler decides whether to
        // drop further.
        knife.path.clear();
        return;
    }

    // Commit on Enter when there's something to apply.
    if keyboard.just_pressed(KeyCode::Enter) && knife.path.len() >= 2 {
        commit_path(
            brush_entity,
            &mut knife,
            &mut brushes,
            &mut bmesh_q,
            &mut history,
        );
        return;
    }

    // Place a new path point on LMB. Snap target is required: if the
    // cursor isn't near a vert / edge, the click is ignored.
    if mouse.just_pressed(MouseButton::Left)
        && let Some(snap) = knife.hover_snap.clone()
    {
        let new_point = KnifePathPoint::from(&snap);
        knife.path.push(new_point);
    }
}

/// Per-frame gizmo overlay: red dots / squares / diamonds for the
/// current snap target, red line segments connecting the path, and a
/// preview segment from the last clicked point to the cursor.
pub(super) fn draw_knife_overlay(
    edit_mode: Res<EditMode>,
    knife: Res<KnifeMode>,
    mut gizmos: Gizmos<BrushOutlineSelectedGizmoGroup>,
) {
    if !matches!(*edit_mode, EditMode::BrushEdit(BrushEditMode::Knife)) {
        return;
    }

    // Path segments.
    for window in knife.path.windows(2) {
        gizmos.line(window[0].world_pos, window[1].world_pos, KNIFE_COLOR);
    }

    // Preview from the last clicked point to the live snap target.
    if let (Some(last), Some(snap)) = (knife.path.last(), knife.hover_snap.as_ref()) {
        gizmos.line(last.world_pos, snap.world_pos, KNIFE_COLOR);
    }

    // Each placed point gets a small red dot so the user sees what
    // they've clicked. Skipped for points that landed on an existing
    // vert (the vert itself reads through the wireframe).
    for point in &knife.path {
        gizmos.sphere(
            Isometry3d::from_translation(point.world_pos),
            default_style::EDIT_VERTEX_RADIUS,
            KNIFE_COLOR,
        );
    }

    // Live snap indicator at the cursor.
    if let Some(snap) = knife.hover_snap.as_ref() {
        match snap.kind {
            KnifeSnapKind::Vertex => {
                // Filled red dot.
                gizmos.sphere(
                    Isometry3d::from_translation(snap.world_pos),
                    default_style::EDIT_VERTEX_RADIUS * 1.2,
                    KNIFE_COLOR,
                );
            }
            KnifeSnapKind::EdgeMidpoint => {
                // Hollow red square.
                draw_square(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::EdgePoint => {
                // Small red diamond on the edge.
                draw_diamond(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::FaceInterior => {
                // Small filled red square at the interior snap target.
                // Visually distinct from `EdgeMidpoint`'s hollow square:
                // a tighter filled marker built from a 4-line cross
                // plus an outline.
                draw_filled_square(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::PathPoint => {
                // Large hollow red sphere outline reads as a circle from
                // any camera angle (gizmos.sphere is wireframe), which
                // makes it distinct from the filled vert dot.
                let r = default_style::EDIT_VERTEX_RADIUS * 1.8;
                gizmos.sphere(Isometry3d::from_translation(snap.world_pos), r, KNIFE_COLOR);
            }
        }
    }
}

fn draw_square(gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>, center: Vec3, color: Color) {
    let size = default_style::EDIT_VERTEX_RADIUS * 1.6;
    // Camera-facing not strictly needed: gizmos always render at the
    // requested world location, and the user reads the indicator as a
    // small marker. A tilted screen-space square would need the
    // camera transform; for MVP the axis-aligned square reads well
    // enough.
    let h = size * 0.5;
    let p0 = center + Vec3::new(-h, -h, 0.0);
    let p1 = center + Vec3::new(h, -h, 0.0);
    let p2 = center + Vec3::new(h, h, 0.0);
    let p3 = center + Vec3::new(-h, h, 0.0);
    gizmos.line(p0, p1, color);
    gizmos.line(p1, p2, color);
    gizmos.line(p2, p3, color);
    gizmos.line(p3, p0, color);
}

/// Filled-looking square: outline plus two diagonal cross-lines. Slightly
/// smaller than `draw_square` so the two glyphs read as distinct.
fn draw_filled_square(
    gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>,
    center: Vec3,
    color: Color,
) {
    let size = default_style::EDIT_VERTEX_RADIUS * 1.2;
    let h = size * 0.5;
    let p0 = center + Vec3::new(-h, -h, 0.0);
    let p1 = center + Vec3::new(h, -h, 0.0);
    let p2 = center + Vec3::new(h, h, 0.0);
    let p3 = center + Vec3::new(-h, h, 0.0);
    gizmos.line(p0, p1, color);
    gizmos.line(p1, p2, color);
    gizmos.line(p2, p3, color);
    gizmos.line(p3, p0, color);
    // Cross-hatch fakes a fill in the wireframe gizmo overlay.
    gizmos.line(p0, p2, color);
    gizmos.line(p1, p3, color);
}

fn draw_diamond(gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>, center: Vec3, color: Color) {
    let size = default_style::EDIT_VERTEX_RADIUS * 1.4;
    let p0 = center + Vec3::new(0.0, size, 0.0);
    let p1 = center + Vec3::new(size, 0.0, 0.0);
    let p2 = center + Vec3::new(0.0, -size, 0.0);
    let p3 = center + Vec3::new(-size, 0.0, 0.0);
    gizmos.line(p0, p1, color);
    gizmos.line(p1, p2, color);
    gizmos.line(p2, p3, color);
    gizmos.line(p3, p0, color);
}

/// Convert a window cursor position to camera-target viewport space,
/// bounds-checking against the supplied viewport UI node.
fn cursor_in_viewport(
    cursor: Vec2,
    camera: &Camera,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec2> {
    let (computed, vp_tf) = viewport_query.get(viewport_entity).ok()?;
    let map = ViewportRemap::new(camera, computed, vp_tf);
    let local = cursor - map.top_left;
    if local.x >= 0.0 && local.y >= 0.0 && local.x <= map.vp_size.x && local.y <= map.vp_size.y {
        Some(local * map.remap)
    } else {
        None
    }
}

/// Pick the closest face of `cache` under the cursor in screen space.
fn pick_face_under_cursor(
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
) -> Option<usize> {
    let mut best_face = None;
    let mut best_depth = f32::MAX;

    for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
        if polygon.len() < 3 {
            continue;
        }
        let screen_verts: Vec<Vec2> = polygon
            .iter()
            .filter_map(|&vi| {
                let world = brush_global.transform_point(cache.vertices[vi]);
                camera.world_to_viewport(cam_tf, world).ok()
            })
            .collect();
        if screen_verts.len() < polygon.len() {
            continue;
        }
        if point_in_polygon_2d(viewport_cursor, &screen_verts) {
            let centroid: Vec3 =
                polygon.iter().map(|&vi| cache.vertices[vi]).sum::<Vec3>() / polygon.len() as f32;
            let world_centroid = brush_global.transform_point(centroid);
            let depth = (cam_tf.translation() - world_centroid).length_squared();
            if depth < best_depth {
                best_depth = depth;
                best_face = Some(face_idx);
            }
        }
    }
    best_face
}

/// Compute the highest-priority snap target on `face_idx` for the
/// current cursor position. Priority order:
///
/// 1. Nearest existing vert (within tolerance) -- always wins.
/// 2. Nearest edge midpoint (Shift held + within tolerance).
/// 3. Nearest point on an edge (within tolerance).
///
/// Returns `None` if no candidate is within tolerance.
fn compute_face_snap(
    face_idx: usize,
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    shift: bool,
) -> Option<KnifeSnapTarget> {
    let polygon = cache.face_polygons.get(face_idx)?;
    if polygon.len() < 3 {
        return None;
    }

    let tolerance =
        window_pixels_to_target_pixels(KNIFE_SNAP_PIXELS, camera, viewport_entity, viewport_query);

    let n = polygon.len();

    // Pass 1: vert snap. If any vert of the face's ring is within
    // tolerance, prefer the closest one (verts beat all edge snaps).
    let mut best_vert: Option<(usize, f32)> = None;
    for i in 0..n {
        let vi = polygon[i];
        let local = cache.vertices[vi];
        let world = brush_global.transform_point(local);
        let Ok(screen) = camera.world_to_viewport(cam_tf, world) else {
            continue;
        };
        let dist = (screen - viewport_cursor).length();
        if dist <= tolerance {
            match best_vert {
                Some((_, prev)) if prev <= dist => {}
                _ => best_vert = Some((vi, dist)),
            }
        }
    }
    if let Some((vi, _)) = best_vert {
        let local = cache.vertices[vi];
        let world = brush_global.transform_point(local);
        return Some(KnifeSnapTarget {
            world_pos: world,
            local_pos: local,
            kind: KnifeSnapKind::Vertex,
            face_idx,
            edge_pair: None,
            vert_idx: Some(vi),
            path_point_idx: None,
        });
    }

    // Pass 2: edge snap. Walk every edge once; for each, find the
    // closest point on the segment and (when Shift is held) the
    // midpoint distance. Keep the best across all edges.
    let mut best_edge_point: Option<(usize, f32, f32)> = None; // (edge_i, t, dist)
    let mut best_midpoint: Option<(usize, f32)> = None; // (edge_i, dist)
    for i in 0..n {
        let a_vi = polygon[i];
        let b_vi = polygon[(i + 1) % n];
        let a_local = cache.vertices[a_vi];
        let b_local = cache.vertices[b_vi];
        let a_world = brush_global.transform_point(a_local);
        let b_world = brush_global.transform_point(b_local);
        let Ok(a_screen) = camera.world_to_viewport(cam_tf, a_world) else {
            continue;
        };
        let Ok(b_screen) = camera.world_to_viewport(cam_tf, b_world) else {
            continue;
        };

        let ab = b_screen - a_screen;
        let len_sq = ab.length_squared();
        let t = if len_sq > 1e-6 {
            ((viewport_cursor - a_screen).dot(ab) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let closest_screen = a_screen + ab * t;
        let dist = (closest_screen - viewport_cursor).length();
        if dist <= tolerance {
            match best_edge_point {
                Some((_, _, prev)) if prev <= dist => {}
                _ => best_edge_point = Some((i, t, dist)),
            }
        }

        if shift {
            let mid_screen = a_screen.lerp(b_screen, 0.5);
            let mid_dist = (mid_screen - viewport_cursor).length();
            if mid_dist <= tolerance {
                match best_midpoint {
                    Some((_, prev)) if prev <= mid_dist => {}
                    _ => best_midpoint = Some((i, mid_dist)),
                }
            }
        }
    }

    // Midpoint wins over edge-point snap when both exist (per spec):
    // Shift "promotes" the edge snap to its midpoint.
    if let Some((edge_i, _)) = best_midpoint {
        let a_vi = polygon[edge_i];
        let b_vi = polygon[(edge_i + 1) % n];
        let a_local = cache.vertices[a_vi];
        let b_local = cache.vertices[b_vi];
        let mid_local = a_local.lerp(b_local, 0.5);
        let mid_world = brush_global.transform_point(mid_local);
        return Some(KnifeSnapTarget {
            world_pos: mid_world,
            local_pos: mid_local,
            kind: KnifeSnapKind::EdgeMidpoint,
            face_idx,
            edge_pair: Some(canonical_edge(a_vi, b_vi)),
            vert_idx: None,
            path_point_idx: None,
        });
    }

    if let Some((edge_i, t, _)) = best_edge_point {
        let a_vi = polygon[edge_i];
        let b_vi = polygon[(edge_i + 1) % n];
        let a_local = cache.vertices[a_vi];
        let b_local = cache.vertices[b_vi];
        let snap_local = a_local.lerp(b_local, t);
        let snap_world = brush_global.transform_point(snap_local);
        return Some(KnifeSnapTarget {
            world_pos: snap_world,
            local_pos: snap_local,
            kind: KnifeSnapKind::EdgePoint,
            face_idx,
            edge_pair: Some(canonical_edge(a_vi, b_vi)),
            vert_idx: None,
            path_point_idx: None,
        });
    }

    None
}

/// Build a `FaceInterior` snap target at the ray-cursor projection
/// onto `face_idx`'s plane. Returns `None` if the ray is parallel to
/// the face or hits behind the camera; caller has already verified the
/// cursor is screen-inside this face.
fn compute_face_interior_snap(
    face_idx: usize,
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
) -> Option<KnifeSnapTarget> {
    let polygon = cache.face_polygons.get(face_idx)?;
    if polygon.len() < 3 {
        return None;
    }

    let ring_local: Vec<Vec3> = polygon.iter().map(|&vi| cache.vertices[vi]).collect();
    let local_normal = jackdaw_geometry::newell_normal(&ring_local);
    if local_normal.length_squared() < 1e-12 {
        return None;
    }
    let (_, brush_rot, _) = brush_global.to_scale_rotation_translation();
    let world_normal = (brush_rot * local_normal).normalize_or_zero();
    if world_normal == Vec3::ZERO {
        return None;
    }
    let centroid_local: Vec3 = ring_local.iter().copied().sum::<Vec3>() / ring_local.len() as f32;
    let centroid_world = brush_global.transform_point(centroid_local);

    let ray = camera.viewport_to_world(cam_tf, viewport_cursor).ok()?;
    let denom = world_normal.dot(*ray.direction);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (centroid_world - ray.origin).dot(world_normal) / denom;
    if t < 0.0 {
        return None;
    }
    let hit_world = ray.origin + *ray.direction * t;

    // Brush-local hit: invert the brush's global transform on the world
    // hit. `transform_point` uses scale + rotation + translation, so
    // we mirror that with affine inverse.
    let affine = brush_global.affine();
    let inv = affine.inverse();
    let local_pos = inv.transform_point3(hit_world);

    Some(KnifeSnapTarget {
        world_pos: hit_world,
        local_pos,
        kind: KnifeSnapKind::FaceInterior,
        face_idx,
        edge_pair: None,
        vert_idx: None,
        path_point_idx: None,
    })
}

/// If the cursor is within `KNIFE_SNAP_PIXELS` of any existing
/// path-point in screen space, build a `PathPoint` snap target
/// referencing that point. We snap in screen space so the indicator
/// feels consistent with vert / edge snaps regardless of camera depth.
fn compute_path_point_snap(
    viewport_cursor: Vec2,
    path: &[KnifePathPoint],
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<KnifeSnapTarget> {
    if path.is_empty() {
        return None;
    }
    let tolerance =
        window_pixels_to_target_pixels(KNIFE_SNAP_PIXELS, camera, viewport_entity, viewport_query);
    let mut best: Option<(usize, f32)> = None;
    for (idx, p) in path.iter().enumerate() {
        let Ok(screen) = camera.world_to_viewport(cam_tf, p.world_pos) else {
            continue;
        };
        let dist = (screen - viewport_cursor).length();
        if dist <= tolerance {
            match best {
                Some((_, prev)) if prev <= dist => {}
                _ => best = Some((idx, dist)),
            }
        }
    }
    let (idx, _) = best?;
    let src = &path[idx];
    Some(KnifeSnapTarget {
        world_pos: src.world_pos,
        local_pos: src.local_pos,
        kind: KnifeSnapKind::PathPoint,
        face_idx: src.face_idx,
        edge_pair: src.edge_pair,
        vert_idx: src.vert_idx,
        path_point_idx: Some(idx),
    })
}

fn window_pixels_to_target_pixels(
    px: f32,
    camera: &Camera,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> f32 {
    let Ok((computed, vp_tf)) = viewport_query.get(viewport_entity) else {
        return px;
    };
    let map = ViewportRemap::new(camera, computed, vp_tf);
    let avg = (map.remap.x.abs() + map.remap.y.abs()) * 0.5;
    if avg > 1e-6 { px * avg } else { px }
}

fn canonical_edge(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Run the queued path as a sequence of face bisections, all wrapped
/// in a single `SetBrush` undo entry.
///
/// Per-point pre-processing:
///   - `FaceInterior` points trigger `face_poke` to insert a center
///     vert on the active face. After a poke, the original face is
///     replaced by N fan triangles; subsequent path points that still
///     reference the now-gone face are relocated to whichever fan
///     triangle now contains them via point-in-triangle on local
///     coordinates.
///   - `PathPoint` re-clicks reuse the original point's resolved vert
///     without re-running geometry ops.
///
/// Per-segment handling:
///   - Same face: bisect with a chord between the two resolved verts
///     (`split_face`).
///   - Adjacent faces sharing exactly one edge: split that edge at the
///     3D segment-edge intersection, bisect each face with the
///     intermediate vert.
///   - Otherwise: warn + skip.
fn commit_path(
    brush_entity: Entity,
    knife: &mut KnifeMode,
    brushes: &mut Query<&mut Brush>,
    bmesh_q: &mut Query<&mut BrushEditMesh>,
    history: &mut ResMut<CommandHistory>,
) {
    let Ok(start_brush) = brushes.get(brush_entity).cloned() else {
        knife.path.clear();
        return;
    };
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        knife.path.clear();
        return;
    };

    // Clone the path out of the resource so we can mutate
    // `bmesh_component` inside the loop without aliasing through
    // `knife.path`. We may also relocate `face_idx` for subsequent
    // points after a `face_poke`, hence the `mut`.
    let mut path: Vec<KnifePathPoint> = knife.path.clone();

    // Resolve each path point to a `VertKey` in advance. This is the
    // single place where `face_poke` runs and where `PathPoint`
    // re-clicks pull the original resolution. Each resolved entry is
    // indexed by the path-point's original position, so segments can
    // look up both endpoints directly.
    let mut resolved: Vec<Option<VertKey>> = vec![None; path.len()];
    for i in 0..path.len() {
        // Replay `PathPoint` references first: if this entry was a
        // re-click on an earlier point, just reuse that point's vert.
        if let Some(source) = path[i].source_path_idx
            && source < i
            && let Some(vk) = resolved[source]
        {
            resolved[i] = Some(vk);
            // Snap face_idx to the source's current (possibly
            // relocated) face so cross-face logic agrees.
            path[i].face_idx = path[source].face_idx;
            continue;
        }

        match path[i].kind {
            KnifeSnapKind::FaceInterior => {
                let face_idx = path[i].face_idx;
                let Some(face_key) =
                    find_face_by_material_idx(&bmesh_component.mesh, face_idx as u32)
                else {
                    warn!(
                        "Knife: face-interior point {} skipped: face {} missing in EditMesh",
                        i, face_idx
                    );
                    continue;
                };
                // Project the click onto the live face plane to keep
                // `face_poke`'s plane check happy after earlier mutations.
                let local_pos = {
                    let face = &bmesh_component.mesh.faces[face_key];
                    let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
                    let mut cur = face.loop_first;
                    for _ in 0..face.loop_count {
                        ring_positions.push(
                            bmesh_component.mesh.verts[bmesh_component.mesh.loops[cur].vert].co,
                        );
                        cur = bmesh_component.mesh.loops[cur].next;
                    }
                    project_onto_face_plane(path[i].local_pos, &ring_positions, face.normal_cache)
                };
                match face_poke(&mut bmesh_component.mesh, face_key, local_pos) {
                    Ok(result) => {
                        resolved[i] = Some(result.center_vert);
                        // Relocate every later path point still
                        // referencing the now-gone original face into
                        // the appropriate fan triangle.
                        relocate_path_points_after_poke(
                            &mut path,
                            i,
                            face_idx,
                            &bmesh_component.mesh,
                            &result.new_faces,
                        );
                    }
                    Err(err) => {
                        warn!("Knife: face_poke for path point {} failed: {:?}", i, err);
                    }
                }
            }
            KnifeSnapKind::Vertex | KnifeSnapKind::EdgeMidpoint | KnifeSnapKind::EdgePoint => {
                match resolve_endpoint(&mut bmesh_component.mesh, &path[i], &start_brush) {
                    Ok(v) => resolved[i] = Some(v),
                    Err(reason) => {
                        warn!("Knife: path point {} resolve failed: {}", i, reason);
                    }
                }
            }
            KnifeSnapKind::PathPoint => {
                // Reached only when `source_path_idx` is missing or
                // out-of-range. Fall through to the geometry kinds we
                // had snap data for (PathPoint copies edge_pair /
                // vert_idx from the source so this still resolves).
                if path[i].vert_idx.is_some() || path[i].edge_pair.is_some() {
                    match resolve_endpoint(&mut bmesh_component.mesh, &path[i], &start_brush) {
                        Ok(v) => resolved[i] = Some(v),
                        Err(reason) => {
                            warn!("Knife: path point {} resolve failed: {}", i, reason);
                        }
                    }
                } else {
                    warn!(
                        "Knife: dangling PathPoint at {} has no resolvable source",
                        i
                    );
                }
            }
        }
    }

    let mut any_applied = false;
    for (segment_idx, pair) in path.windows(2).enumerate() {
        let idx0 = segment_idx;
        let idx1 = segment_idx + 1;
        let Some(v0) = resolved[idx0] else { continue };
        let Some(v1) = resolved[idx1] else { continue };
        if v0 == v1 {
            // Reuse / self-loop. No geometry to create.
            continue;
        }
        let p0 = &pair[0];
        let p1 = &pair[1];

        if p0.face_idx == p1.face_idx {
            match bisect_same_face_with_verts(&mut bmesh_component.mesh, v0, v1, p0.face_idx) {
                Ok(_) => any_applied = true,
                Err(reason) => warn!("Knife: segment {} skipped: {}", segment_idx, reason),
            }
        } else {
            // Cross-face: try shared-edge handling.
            match bisect_cross_face(
                &mut bmesh_component.mesh,
                p0.face_idx,
                p1.face_idx,
                p0.world_pos,
                p1.world_pos,
                p0.local_pos,
                p1.local_pos,
                v0,
                v1,
            ) {
                Ok(_) => any_applied = true,
                Err(reason) => warn!(
                    "Knife: cross-face segment {} ({} -> {}) skipped: {}",
                    segment_idx, p0.face_idx, p1.face_idx, reason
                ),
            }
        }
    }

    if !any_applied {
        knife.path.clear();
        return;
    }

    // Re-cache normals on every face (split_face changes ring shape).
    let face_keys_all: Vec<_> = bmesh_component.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &bmesh_component.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &bmesh_component.mesh.loops[cur];
            ring_positions.push(bmesh_component.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        bmesh_component.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten + sync brush.
    let new_topology = bmesh_component.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        knife.path.clear();
        return;
    };

    // Extend brush.faces for newly split faces. Use the first cut's
    // face as a template fallback (mirrors the modal version).
    let template_idx = knife.path.first().map(|p| p.face_idx).unwrap_or(0);
    while brush.faces.len() < new_topology.polygons.len() {
        let template = start_brush
            .faces
            .get(template_idx)
            .cloned()
            .or_else(|| brush.faces.last().cloned())
            .unwrap_or_default();
        brush.faces.push(template);
    }

    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (idx, face_data) in brush.faces.iter_mut().enumerate() {
        if idx < new_topology.polygons.len() {
            let normal = new_topology.face_normal_with(&positions, idx);
            let v0_idx =
                new_topology.loops[new_topology.polygons[idx].loop_start as usize].vert as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
        }
    }
    brush.topology = new_topology;

    // Re-lift EditMesh so vert_keys / face_keys stay consistent with
    // the flattened brush topology.
    let new_bmesh = EditMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_bmesh.verts.keys().collect();
    let mut new_face_keys: Vec<FaceKey> = vec![FaceKey::default(); new_bmesh.faces.len()];
    for (k, f) in new_bmesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    bmesh_component.mesh = new_bmesh;
    bmesh_component.vert_keys = new_vert_keys;
    bmesh_component.face_keys = new_face_keys;

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: start_brush,
        new: brush.clone(),
        label: "Knife".to_string(),
    }));

    knife.path.clear();
}

/// Bisect `face_idx` with a chord between two already-resolved verts on
/// its ring. Caller must have run any `face_poke` / `split_edge` pre-ops
/// so both `v0` and `v1` exist on the live face ring.
fn bisect_same_face_with_verts(
    bmesh: &mut EditMesh,
    v0: VertKey,
    v1: VertKey,
    face_idx: usize,
) -> Result<(), &'static str> {
    if v0 == v1 {
        return Err("endpoints resolved to the same vert");
    }
    // Find the face that actually contains both verts. After a
    // `face_poke`, the original `material_idx`-stable face may have
    // become several fan triangles; we search for the one whose ring
    // contains both endpoints. Falls back to the original
    // material_idx lookup if the search comes up empty.
    let face_key = face_containing_verts(bmesh, v0, v1)
        .or_else(|| find_face_by_material_idx(bmesh, face_idx as u32))
        .ok_or("face missing in EditMesh")?;
    if !face_ring_contains(bmesh, face_key, v0) || !face_ring_contains(bmesh, face_key, v1) {
        return Err("verts not both on face ring");
    }
    split_face(bmesh, face_key, v0, v1).map_err(|_| "split_face failed")?;
    Ok(())
}

/// Cross-face segment: the two endpoints sit on different faces. Find
/// the single shared edge between those faces, split it at the 3D
/// intersection of the segment with the shared edge, then bisect each
/// face with the new intermediate vert.
#[allow(clippy::too_many_arguments)]
fn bisect_cross_face(
    bmesh: &mut EditMesh,
    face_idx_0: usize,
    face_idx_1: usize,
    p0_world: Vec3,
    p1_world: Vec3,
    p0_local: Vec3,
    p1_local: Vec3,
    v0: VertKey,
    v1: VertKey,
) -> Result<(), &'static str> {
    // The endpoints will be on faces tied to `face_idx_0` and
    // `face_idx_1` either by `material_idx` (no earlier pokes) or by
    // ring membership (poked face was split into fan triangles).
    let face_a = face_containing_vert(bmesh, v0)
        .or_else(|| find_face_by_material_idx(bmesh, face_idx_0 as u32))
        .ok_or("face A missing in EditMesh")?;
    let face_b = face_containing_vert(bmesh, v1)
        .or_else(|| find_face_by_material_idx(bmesh, face_idx_1 as u32))
        .ok_or("face B missing in EditMesh")?;
    if face_a == face_b {
        return Err("both endpoints on the same face after relocation");
    }

    let shared = shared_edges(bmesh, face_a, face_b);
    if shared.is_empty() {
        return Err("no shared edge");
    }
    if shared.len() > 1 {
        return Err("multiple shared edges; not supported");
    }
    let shared_edge = shared[0];

    // Compute the parametric position on the shared edge closest to
    // the 3D line through (p0, p1). We use brush-local coordinates
    // throughout: the edge verts already store local positions in
    // `bmesh`, and `p*_local` came from the original click ray cast.
    // World-space `p*_world` is kept as a fallback for diagnostics but
    // the actual line-line solve runs in local space because that's
    // where `split_edge` will place the new vert.
    let ev0 = bmesh.edges[shared_edge].v[0];
    let ev1 = bmesh.edges[shared_edge].v[1];
    let edge_a = bmesh.verts[ev0].co;
    let edge_b = bmesh.verts[ev1].co;

    let t = line_segment_crossing_param(p0_local, p1_local, edge_a, edge_b)
        .ok_or("segment-edge intersection degenerate")?;
    let _ = p0_world;
    let _ = p1_world;

    let split_t = t.clamp(0.001, 0.999);
    let inter_vert = split_edge(bmesh, shared_edge, split_t).map_err(|_| "split_edge failed")?;

    // Both halves of the cross-face cut bisect their respective face
    // with the new intermediate vert. Either face may have lost its
    // material_idx-stable identity to an earlier `face_poke`; locate
    // them by ring membership of the relevant endpoint.
    let face_a_now = face_containing_verts(bmesh, v0, inter_vert)
        .ok_or("face A no longer contains v0 + intermediate")?;
    split_face(bmesh, face_a_now, v0, inter_vert).map_err(|_| "split_face A failed")?;

    let face_b_now = face_containing_verts(bmesh, v1, inter_vert)
        .ok_or("face B no longer contains v1 + intermediate")?;
    split_face(bmesh, face_b_now, v1, inter_vert).map_err(|_| "split_face B failed")?;

    Ok(())
}

/// 3D line intersection projected onto whichever plane keeps the math
/// well-conditioned. Returns the parameter `t` along the edge
/// `(edge_a, edge_b)` at which the segment `(p0, p1)` crosses it.
///
/// Algorithm: build an orthonormal basis at the edge midpoint with the
/// edge direction as one axis and the projection of `(p1 - p0)` onto
/// the plane perpendicular to the edge as the other. Project all four
/// points into that 2D basis, then run standard 2D line-line
/// intersection.
fn line_segment_crossing_param(p0: Vec3, p1: Vec3, edge_a: Vec3, edge_b: Vec3) -> Option<f32> {
    let edge_dir = edge_b - edge_a;
    let edge_len = edge_dir.length();
    if edge_len < 1e-6 {
        return None;
    }
    let u = edge_dir / edge_len;
    let seg_dir = p1 - p0;
    if seg_dir.length_squared() < 1e-12 {
        return None;
    }
    // Pick the second basis axis as the seg-dir component perpendicular
    // to the edge. If the segment is exactly parallel to the edge, the
    // crossing is degenerate (or coincident); bail.
    let seg_perp = seg_dir - u * seg_dir.dot(u);
    if seg_perp.length_squared() < 1e-12 {
        return None;
    }
    let v = seg_perp.normalize();
    let origin = edge_a;
    let to2 = |p: Vec3| -> Vec2 {
        let d = p - origin;
        Vec2::new(d.dot(u), d.dot(v))
    };
    let a2 = to2(edge_a);
    let b2 = to2(edge_b);
    let p02 = to2(p0);
    let p12 = to2(p1);
    // Solve `a2 + t * (b2 - a2) == p02 + s * (p12 - p02)`.
    let r = b2 - a2;
    let s = p12 - p02;
    let denom = r.x * s.y - r.y * s.x;
    if denom.abs() < 1e-9 {
        return None;
    }
    let q = p02 - a2;
    let t = (q.x * s.y - q.y * s.x) / denom;
    Some(t)
}

/// Project `point` onto the plane defined by `ring` + `normal`. The
/// face plane anchor is the ring centroid (matching `face_poke`'s
/// `PointNotInFacePlane` check).
fn project_onto_face_plane(point: Vec3, ring: &[Vec3], normal: Vec3) -> Vec3 {
    if ring.is_empty() || normal == Vec3::ZERO {
        return point;
    }
    let centroid: Vec3 = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return point;
    }
    let signed = n.dot(point - centroid);
    point - n * signed
}

/// Walk every face in the mesh and return one whose ring contains
/// both `va` and `vb`. Used for resolving the live face after earlier
/// `face_poke` / `split_face` ops have churned `material_idx`-stable
/// identities.
fn face_containing_verts(bmesh: &EditMesh, va: VertKey, vb: VertKey) -> Option<FaceKey> {
    bmesh
        .faces
        .iter()
        .find(|(k, _)| face_ring_contains(bmesh, *k, va) && face_ring_contains(bmesh, *k, vb))
        .map(|(k, _)| k)
}

/// Walk every face in the mesh and return one whose ring contains
/// `va`. Returns the first match; for the knife pipeline that's the
/// face the segment originated from (re-resolved post-poke).
fn face_containing_vert(bmesh: &EditMesh, va: VertKey) -> Option<FaceKey> {
    bmesh
        .faces
        .iter()
        .find(|(k, _)| face_ring_contains(bmesh, *k, va))
        .map(|(k, _)| k)
}

fn face_ring_contains(bmesh: &EditMesh, face: FaceKey, target: VertKey) -> bool {
    let f = &bmesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        if bmesh.loops[cur].vert == target {
            return true;
        }
        cur = bmesh.loops[cur].next;
    }
    false
}

/// Return every edge shared between `face_a` and `face_b`. For two
/// adjacent faces of a closed brush this is exactly one edge; if the
/// faces aren't adjacent the result is empty.
fn shared_edges(bmesh: &EditMesh, face_a: FaceKey, face_b: FaceKey) -> Vec<EdgeKey> {
    let edges_a = face_edges(bmesh, face_a);
    let edges_b = face_edges(bmesh, face_b);
    edges_a
        .into_iter()
        .filter(|e| edges_b.contains(e))
        .collect()
}

fn face_edges(bmesh: &EditMesh, face: FaceKey) -> Vec<EdgeKey> {
    let f = &bmesh.faces[face];
    let mut edges = Vec::with_capacity(f.loop_count as usize);
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        edges.push(bmesh.loops[cur].edge);
        cur = bmesh.loops[cur].next;
    }
    edges
}

/// After a `face_poke` on `original_face_idx`, the original face
/// disappears and its fan triangles take its place. Path points that
/// still reference `original_face_idx` need to be relocated to
/// whichever fan triangle now contains them. We use the path point's
/// `local_pos` projected onto the new triangle's plane for a 2D
/// point-in-triangle test.
///
/// The relocated entry keeps its kind (face_interior / edge / vert)
/// and its snap metadata; only `face_idx` changes, and only when the
/// point sat on the same `material_idx` as the poked face.
fn relocate_path_points_after_poke(
    path: &mut [KnifePathPoint],
    started_at: usize,
    original_face_idx: usize,
    bmesh: &EditMesh,
    new_faces: &[FaceKey],
) {
    if new_faces.is_empty() {
        return;
    }
    // Collect ring positions + material_idx for each candidate face so
    // we can walk through them once per relocation.
    let mut candidates: Vec<(u32, Vec<Vec3>, Vec3)> = Vec::with_capacity(new_faces.len());
    for &fk in new_faces {
        let face = &bmesh.faces[fk];
        let mut ring = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            ring.push(bmesh.verts[bmesh.loops[cur].vert].co);
            cur = bmesh.loops[cur].next;
        }
        candidates.push((face.material_idx, ring, face.normal_cache));
    }

    for entry in path.iter_mut().skip(started_at + 1) {
        if entry.face_idx != original_face_idx {
            continue;
        }
        for (mat_idx, ring, normal) in &candidates {
            if point_in_face_triangle(entry.local_pos, ring, *normal) {
                entry.face_idx = *mat_idx as usize;
                break;
            }
        }
    }
}

/// Project `point` onto the face plane and run point-in-polygon in 2D
/// local coords. Robust to convex / concave; we only call this for fan
/// triangles (3 verts) so the polygon check is trivially valid.
fn point_in_face_triangle(point: Vec3, ring: &[Vec3], normal: Vec3) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return false;
    }
    // Build a 2D basis on the face plane.
    let (u_axis, v_axis) = jackdaw_geometry::compute_face_tangent_axes(n);
    let origin = ring[0];
    let to2 = |p: Vec3| -> Vec2 {
        let d = p - origin;
        Vec2::new(d.dot(u_axis), d.dot(v_axis))
    };
    let poly: Vec<Vec2> = ring.iter().map(|&r| to2(r)).collect();
    crate::viewport_util::point_in_polygon_2d(to2(point), &poly)
}

/// Resolve a path point to a `VertKey` in `bmesh`, edge-splitting if
/// necessary. Position-matches the snapshot brush vert against the
/// live EditMesh because edge splits / face splits preserve existing
/// vert positions.
fn resolve_endpoint(
    bmesh: &mut EditMesh,
    point: &KnifePathPoint,
    start_brush: &Brush,
) -> Result<VertKey, &'static str> {
    if let Some(vi) = point.vert_idx {
        let target = start_brush
            .topology
            .vertices
            .get(vi)
            .map(|v| v.position)
            .ok_or("vert index out of range")?;
        return find_vert_by_position(bmesh, target).ok_or("vert not found in EditMesh");
    }

    let (a_vi, b_vi) = point.edge_pair.ok_or("path point has no edge or vert")?;
    let a_pos = start_brush
        .topology
        .vertices
        .get(a_vi)
        .map(|v| v.position)
        .ok_or("edge a vert index out of range")?;
    let b_pos = start_brush
        .topology
        .vertices
        .get(b_vi)
        .map(|v| v.position)
        .ok_or("edge b vert index out of range")?;
    let va = find_vert_by_position(bmesh, a_pos).ok_or("edge a vert missing in EditMesh")?;
    let vb = find_vert_by_position(bmesh, b_pos).ok_or("edge b vert missing in EditMesh")?;
    let ek = find_edge_between(bmesh, va, vb).ok_or("edge missing in EditMesh")?;

    let v0 = bmesh.edges[ek].v[0];
    let v1 = bmesh.edges[ek].v[1];
    let p0 = bmesh.verts[v0].co;
    let p1 = bmesh.verts[v1].co;
    let dir = p1 - p0;
    let len_sq = dir.length_squared();
    let t = if len_sq > 1e-6 {
        ((point.local_pos - p0).dot(dir) / len_sq).clamp(0.0, 1.0)
    } else {
        0.5
    };

    split_edge(bmesh, ek, t).map_err(|_| "split_edge failed")
}

fn find_edge_between(bmesh: &EditMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

/// Find the vert with the smallest squared distance to `target`. The
/// modal version of knife required exact-match (epsilon 1e-6), and
/// every snap on the live brush is a snapshot of an exact topology
/// vert position, so that tolerance still holds.
fn find_vert_by_position(bmesh: &EditMesh, target: Vec3) -> Option<VertKey> {
    let mut best: Option<(VertKey, f32)> = None;
    for (k, v) in bmesh.verts.iter() {
        let d = (v.co - target).length_squared();
        match best {
            Some((_, prev)) if prev <= d => {}
            _ => best = Some((k, d)),
        }
    }
    let (k, d) = best?;
    if d <= 1e-6 { Some(k) } else { None }
}

fn find_face_by_material_idx(bmesh: &EditMesh, idx: u32) -> Option<FaceKey> {
    bmesh
        .faces
        .iter()
        .find(|(_, f)| f.material_idx == idx)
        .map(|(k, _)| k)
}
