//! Knife edit mode: incremental, multi-segment face bisection.
//!
//! Same shape as `BrushEditMode::Clip`. Pressing `K` with a brush
//! selected enters Knife mode; pressing `K` again or Escape exits and
//! discards any in-progress path.
//!
//! Each frame, the cursor snaps to the closest snap target within a
//! small tolerance. Priority order (highest first):
//!
//! - **Path-point snap**: any point already placed in the current path
//!   (large hollow red circle, outlined in white). Lets the user close
//!   loops.
//! - **Vertex snap**: solid filled red dot.
//! - **Edge midpoint snap** (Shift held): hollow red diamond.
//! - **Grid point snap** (translate snap active): red plus / cross.
//! - **Edge point snap**: small filled red square.
//! - **Face interior snap**: red X / cross. The cursor is over a face
//!   but not near any vert or edge; commit will add a Steiner point at
//!   this position and retriangulate the face.
//!
//! Vert and edge snap candidates are filtered by camera depth (clip-space
//! z): the candidate with the smallest depth wins, with screen distance
//! as the tiebreaker. This prevents the cursor from snapping to verts /
//! edges that live on a back-facing face occluded by the front face the
//! user is actually pointing at.
//!
//! LMB click commits the current snap target into the path. Enter
//! dispatches the `brush.knife.commit` operator, which bisects each
//! adjacent pair of points and is captured by the framework as a
//! single snapshot-diff undo entry. Esc or RMB cancels the in-progress
//! path.
//!
//! Commit handling: the path is applied to the `HalfedgeMesh` using
//! **topology mutations only** (no CDT). The pipeline mirrors how
//! the knife tool routes a multi-segment cut across faces.
//!
//! First, walk every path point in order, resolving each to a live
//! `VertKey` in the post-mutation mesh:
//!
//! - Vertex snap: position-lookup against the live mesh.
//! - Edge snap (`EdgePoint`, `EdgeMidpoint`): find a live edge whose
//!   endpoints flank the click position, then `split_edge` it. This
//!   handles the case where an earlier mutation has already split the
//!   original edge into two sub-edges; the search picks the live
//!   sub-edge that contains the click.
//! - Interior / Grid snap: pick the live face containing the click
//!   (point-in-polygon in the face plane) and `face_poke` it. Subsequent
//!   path points on the same original face look up which fan tri is
//!   alive now and route through it.
//! - Path-point reclick: inherits the resolved `VertKey` from its source.
//!
//! Then for each consecutive pair, perform a chord:
//!
//! - Both verts on the same live face's ring: `split_face`.
//! - Different live faces: cross-face routing. Find a face adjacent to
//!   `va` whose boundary the segment crosses, `split_edge` at the
//!   crossing, then recurse with the intermediate as the new `va`.
//!
//! If a segment can't be resolved cleanly (no path, adjacent verts that
//! can't chord, or already-collapsed pair), it's logged and skipped but
//! the commit continues. Catastrophic failures (rare) restore from the
//! pre-commit snapshot.
//!
//! Cut-through (project the cut through the whole brush
//! so the back face is cut simultaneously) is filed as task #97.

use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::edge_split::split_edge;
use jackdaw_geometry::halfedge::ops::face_poke::face_poke;
use jackdaw_geometry::halfedge::ops::face_split::split_face;
use jackdaw_geometry::halfedge::{EdgeKey, FaceKey, HalfedgeMesh, LoopKey, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushMeshCache, BrushSelection, EditMode};
use crate::default_style;
use crate::face_grid::BrushOutlineSelectedGizmoGroup;
use crate::viewport::{ActiveViewport, MainViewportCamera, SceneViewport};
use crate::viewport_util::{ViewportRemap, point_in_polygon_2d};

/// Screen-space pixel tolerance for snapping the cursor onto a vert, an
/// edge midpoint, or an edge interior. Specified in window logical
/// pixels; converted to camera-target pixels per frame so `HiDPI` and
/// fractional UI scaling don't shrink the snap region.
const KNIFE_SNAP_PIXELS: f32 = 12.0;

/// Red used for every knife visual (matches the draw-brush cut-mode
/// color in `default_style`).
const KNIFE_COLOR: Color = Color::srgb(1.0, 0.2, 0.2);

/// White outline used to make the `PathPoint` marker pop against the
/// red wireframe and red preview line.
const KNIFE_PATH_POINT_OUTLINE: Color = Color::srgb(1.0, 1.0, 1.0);

/// Camera-depth tie tolerance for snap candidate ordering. Two candidates
/// whose `clip_z` differ by less than this are treated as equally deep
/// and the screen-distance tiebreaker wins. Set small enough that the
/// front face wins decisively over a back face on a normal-sized brush.
const DEPTH_TIE_EPSILON: f32 = 1e-4;

/// Kind of snap target chosen for the current cursor position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnifeSnapKind {
    /// Snapped to an existing vert within tolerance.
    Vertex,
    /// Snapped to an edge midpoint (Shift held + within edge tolerance).
    EdgeMidpoint,
    /// Snapped to a grid point on the face plane (translate snap active).
    GridPoint,
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
/// resolve the corresponding `VertKey` in a freshly-lifted `HalfedgeMesh`
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

/// Knife edit-mode state. Same shape as `ClipState`: cleared
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
    /// World-space points where the current path's segments cross
    /// existing mesh edges. Predicted every frame from the path + live
    /// cursor; rendered as small markers so the user can see, before
    /// committing, which existing edges the cut will split. Mirrors
    /// live edge-crossing preview dots during drag.
    pub preview_intersections: Vec<Vec3>,
    /// Points popped off `path` by in-modal Ctrl+Z, kept so Ctrl+Shift+Z
    /// can re-add them. Cleared when a new point is placed (a fresh
    /// branch invalidates the redo trail) or when the modal exits.
    pub undone_path: Vec<KnifePathPoint>,
}

impl KnifeMode {
    pub fn clear(&mut self) {
        self.brush_entity = None;
        self.path.clear();
        self.hover_snap = None;
        self.preview_intersections.clear();
        self.undone_path.clear();
    }

    /// Pop the most recently placed path point onto the redo stack.
    /// Returns true if a point was popped.
    pub fn undo_point(&mut self) -> bool {
        if let Some(p) = self.path.pop() {
            self.undone_path.push(p);
            true
        } else {
            false
        }
    }

    /// Re-add the most recently undone path point. Returns true if a
    /// point was re-added.
    pub fn redo_point(&mut self) -> bool {
        if let Some(p) = self.undone_path.pop() {
            self.path.push(p);
            true
        } else {
            false
        }
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushKnifeCommitOp>();
}

/// Commit the queued knife path. Dispatched by `handle_knife_mode` on
/// Enter so the snapshot-based undo path captures the cut alongside
/// every other edit, instead of pushing a standalone `SetBrush` command.
#[operator(
    id = "brush.knife.commit",
    label = "Knife: Commit Cut",
    allows_undo = true
)]
pub(crate) fn brush_knife_commit(
    _: In<OperatorParameters>,
    knife: Res<KnifeMode>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(brush_entity) = knife.brush_entity else {
        return OperatorResult::Cancelled;
    };
    if knife.path.len() < 2 {
        return OperatorResult::Cancelled;
    }
    commands.queue(move |world: &mut World| {
        commit_path(world, brush_entity);
    });
    OperatorResult::Finished
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
    cursor: crate::viewport::UiCursorPos,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainViewportCamera>>,
    viewport_query: Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    brush_caches: Query<&BrushMeshCache>,
    brush_transforms: Query<&GlobalTransform>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
    mut knife: ResMut<KnifeMode>,
    snap_settings: Res<crate::snapping::SnapSettings>,
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
    let Some(cursor_pos) = cursor.get() else {
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

    // Resolve snap. Priority order (highest first):
    //   1. Existing vert within tolerance.
    //   2. Path-point reuse (re-click on an existing path point).
    //   3. Edge midpoint (Shift held).
    //   4. Grid point on the face plane (when translate snap is active).
    //   5. Edge point.
    //   6. Face interior.
    //
    // Vert and edge snaps are scanned across ALL face edges (deduplicated
    // by canonical vert pair) so the snap dot stays put when the cursor
    // crosses the boundary between two faces in screen space; relying
    // solely on `pick_face_under_cursor` for the snap face was the cause
    // of the "snap dot disappears near edges" bug.
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let path_snap = compute_path_point_snap(
        viewport_cursor,
        &knife.path,
        camera,
        cam_tf,
        viewport_entity,
        &viewport_query,
    );

    let hover_face = pick_face_under_cursor(viewport_cursor, cache, brush_global, camera, cam_tf);

    let vert_snap = compute_vert_snap_all(
        viewport_cursor,
        cache,
        brush_global,
        camera,
        cam_tf,
        viewport_entity,
        &viewport_query,
        hover_face,
    );

    let (edge_mid_snap, edge_point_snap) = compute_edge_snap_all(
        viewport_cursor,
        cache,
        brush_global,
        camera,
        cam_tf,
        viewport_entity,
        &viewport_query,
        hover_face,
        shift,
    );

    // Grid snap: only when a face is under the cursor (we need a plane
    // to project the cursor onto, and a face to attach the snap to).
    let grid_snap = if snap_settings.translate_active(ctrl)
        && snap_settings.translate_increment > 0.0
        && let Some(face_idx) = hover_face
    {
        compute_grid_snap(
            face_idx,
            viewport_cursor,
            cache,
            brush_global,
            camera,
            cam_tf,
            viewport_entity,
            &viewport_query,
            snap_settings.translate_increment,
        )
    } else {
        None
    };

    // Fall back to face-interior snap when nothing else stuck but the
    // cursor is over a face. We project a ray through the cursor onto
    // the face plane to get a stable world-space point.
    let interior_snap = if let Some(face_idx) = hover_face {
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

    knife.hover_snap = vert_snap
        .or(path_snap)
        .or(edge_mid_snap)
        .or(grid_snap)
        .or(edge_point_snap)
        .or(interior_snap);

    // Predict where the current path will introduce new verts on
    // existing mesh edges so the user sees crossings before committing.
    knife.preview_intersections = compute_path_edge_intersections(
        &knife.path,
        knife.hover_snap.as_ref(),
        cache,
        brush_global,
        camera,
        cam_tf,
    );

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

    // Commit on Enter when there's something to apply. Dispatching via
    // the operator framework so the cut is captured by the snapshot-based
    // undo path alongside every other edit, instead of pushing a
    // standalone `SetBrush` command from inside this system.
    if keyboard.just_pressed(KeyCode::Enter) && knife.path.len() >= 2 {
        commands
            .operator(BrushKnifeCommitOp::ID)
            .settings(CallOperatorSettings {
                creates_history_entry: true,
                ..default()
            })
            .call();
        return;
    }

    // Place a new path point on LMB. Snap target is required: if the
    // cursor isn't near a vert / edge, the click is ignored. Placing a
    // new point starts a fresh history branch, so any pending redo
    // points are discarded.
    if mouse.just_pressed(MouseButton::Left)
        && let Some(snap) = knife.hover_snap.clone()
    {
        let new_point = KnifePathPoint::from(&snap);
        knife.path.push(new_point);
        knife.undone_path.clear();
    }
}

/// Per-frame gizmo overlay: a per-kind glyph (dot, diamond, plus,
/// filled square, X, or white-outlined circle) for the current snap
/// target, red line segments connecting the path, and a preview segment
/// from the last clicked point to the cursor.
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

    // Preview verts at predicted edge crossings. Slightly smaller and hollow so they read as "will be
    // created" rather than "already placed".
    for &crossing in &knife.preview_intersections {
        let r = default_style::EDIT_VERTEX_RADIUS * 0.7;
        gizmos.sphere(Isometry3d::from_translation(crossing), r, KNIFE_COLOR);
    }

    // Live snap indicator at the cursor. Each kind gets a distinct
    // glyph so the user can tell at a glance what the click will lock
    // onto: vert / edge midpoint / edge / grid / face interior / path
    // point.
    if let Some(snap) = knife.hover_snap.as_ref() {
        match snap.kind {
            KnifeSnapKind::Vertex => {
                // Solid red circle: filled sphere reads as a dot.
                gizmos.sphere(
                    Isometry3d::from_translation(snap.world_pos),
                    default_style::EDIT_VERTEX_RADIUS * 1.2,
                    KNIFE_COLOR,
                );
            }
            KnifeSnapKind::EdgeMidpoint => {
                // Hollow red diamond.
                draw_diamond(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::GridPoint => {
                // Red plus / cross: distinguishes grid snap from
                // edge and vert markers.
                draw_plus(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::EdgePoint => {
                // Small filled red square on the edge.
                draw_filled_square(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::FaceInterior => {
                // Red X / cross: visually distinct from `GridPoint`'s
                // axis-aligned plus.
                draw_x(&mut gizmos, snap.world_pos, KNIFE_COLOR);
            }
            KnifeSnapKind::PathPoint => {
                // Hollow red circle outlined in white: the white ring
                // makes the path-point marker pop against red path
                // lines so the user can tell they're snapping back to
                // an already-placed point.
                let r = default_style::EDIT_VERTEX_RADIUS * 1.8;
                let outer = r * 1.25;
                gizmos.sphere(
                    Isometry3d::from_translation(snap.world_pos),
                    outer,
                    KNIFE_PATH_POINT_OUTLINE,
                );
                gizmos.sphere(Isometry3d::from_translation(snap.world_pos), r, KNIFE_COLOR);
            }
        }
    }
}

/// Red X / cross used for the face-interior snap indicator. Two
/// diagonal line segments centered on `center`. Visually distinct from
/// `draw_plus`, which is axis-aligned.
fn draw_x(gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>, center: Vec3, color: Color) {
    let size = default_style::EDIT_VERTEX_RADIUS * 1.6;
    let h = size * 0.5;
    gizmos.line(
        center + Vec3::new(-h, -h, 0.0),
        center + Vec3::new(h, h, 0.0),
        color,
    );
    gizmos.line(
        center + Vec3::new(-h, h, 0.0),
        center + Vec3::new(h, -h, 0.0),
        color,
    );
}

/// Filled-looking square: outline plus two diagonal cross-lines. Slightly
/// smaller than the edge-midpoint diamond so the two glyphs read as
/// distinct.
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

/// Red plus / cross used for grid-snap indicators. Two perpendicular
/// line segments centered on `center`, axis-aligned in world space.
fn draw_plus(gizmos: &mut Gizmos<BrushOutlineSelectedGizmoGroup>, center: Vec3, color: Color) {
    let size = default_style::EDIT_VERTEX_RADIUS * 1.6;
    let h = size * 0.5;
    gizmos.line(
        center + Vec3::new(-h, 0.0, 0.0),
        center + Vec3::new(h, 0.0, 0.0),
        color,
    );
    gizmos.line(
        center + Vec3::new(0.0, -h, 0.0),
        center + Vec3::new(0.0, h, 0.0),
        color,
    );
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

/// Maximum 3D distance (squared, world units) between a cut-path point
/// and a brush edge at the same screen-space intersection for the
/// crossing to be considered a real 3D hit. Smaller values reject more
/// false positives (e.g. a top-face cut that visually crosses a side-
/// face edge in projection); too small and legitimate cross-face cuts
/// get dropped. 0.05^2 ~= 5 cm^2 works well at typical level scale.
const PREVIEW_INTERSECT_TOL_SQ: f32 = 0.05 * 0.05;

/// Predict where the in-progress knife path (path points + live cursor)
/// will introduce new verts on the brush's existing edges. Used purely
/// for preview drawing: every consecutive pair of path points (plus the
/// last-to-cursor segment) is intersected against each unique mesh edge
/// in screen space; for hits, the 3D crossing point on the mesh edge is
/// returned. /// user can see, before committing, which existing edges the cut will
/// split.
///
/// Screen-space intersection (not full 3D) keeps this cheap: the brush's
/// `face_polygons` + vertex cache is already in screen-projection range,
/// and the resulting 3D point is computed from the *edge* parameter (so
/// the dot sits on the actual mesh edge, not floating in space).
fn compute_path_edge_intersections(
    path: &[KnifePathPoint],
    cursor_snap: Option<&KnifeSnapTarget>,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::new();
    if path.is_empty() {
        return out;
    }
    let mut world_pts: Vec<Vec3> = path.iter().map(|p| p.world_pos).collect();
    if let Some(snap) = cursor_snap
        && path.last().map(|p| p.world_pos) != Some(snap.world_pos)
    {
        world_pts.push(snap.world_pos);
    }
    if world_pts.len() < 2 {
        return out;
    }

    let screen_pts: Vec<Vec2> = world_pts
        .iter()
        .filter_map(|&p| camera.world_to_viewport(cam_tf, p).ok())
        .collect();
    if screen_pts.len() != world_pts.len() {
        return out;
    }

    let mut seen_edges: std::collections::HashSet<(usize, usize)> = Default::default();
    for polygon in &cache.face_polygons {
        if polygon.len() < 3 {
            continue;
        }
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            if !seen_edges.insert(key) {
                continue;
            }
            let world_a = brush_global.transform_point(cache.vertices[a]);
            let world_b = brush_global.transform_point(cache.vertices[b]);
            let Ok(screen_a) = camera.world_to_viewport(cam_tf, world_a) else {
                continue;
            };
            let Ok(screen_b) = camera.world_to_viewport(cam_tf, world_b) else {
                continue;
            };

            for j in 0..screen_pts.len() - 1 {
                let seg_a = screen_pts[j];
                let seg_b = screen_pts[j + 1];
                if let Some((t_seg, t_edge)) =
                    segment_intersect_2d(seg_a, seg_b, screen_a, screen_b)
                {
                    // Depth-filter: a screen-space intersection only
                    // corresponds to a real 3D crossing if the 3D points
                    // along the two segments are coincident at the
                    // intersection parameter. For a cut on the top face
                    // that *appears* to cross a side-face edge in
                    // projection, the 3D points are far apart (one on
                    // top, one on the side); skip those.
                    let p_cut = world_pts[j].lerp(world_pts[j + 1], t_seg);
                    let p_edge = world_a.lerp(world_b, t_edge);
                    if p_cut.distance_squared(p_edge) > PREVIEW_INTERSECT_TOL_SQ {
                        continue;
                    }
                    let crossing = p_edge;
                    let on_endpoint = world_pts
                        .iter()
                        .any(|p| p.distance_squared(crossing) < 1e-8);
                    if !on_endpoint {
                        out.push(crossing);
                    }
                }
            }
        }
    }
    out
}

/// 2D segment-segment intersection. Returns `(t1, t2)` where both
/// parameters are in `[0, 1]`, or `None` if the segments do not cross.
fn segment_intersect_2d(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> Option<(f32, f32)> {
    let d1 = p2 - p1;
    let d2 = p4 - p3;
    let denom = d1.x * d2.y - d1.y * d2.x;
    if denom.abs() < 1e-6 {
        return None;
    }
    let dx = p3.x - p1.x;
    let dy = p3.y - p1.y;
    let t1 = (dx * d2.y - dy * d2.x) / denom;
    let t2 = (dx * d1.y - dy * d1.x) / denom;
    if !(0.0..=1.0).contains(&t1) || !(0.0..=1.0).contains(&t2) {
        return None;
    }
    Some((t1, t2))
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

/// Scan every face polygon's verts. Returns the closest vert within
/// `KNIFE_SNAP_PIXELS` of the cursor, deduplicated by vert index so a
/// shared vert isn't double-counted across faces.
///
/// Candidates are ordered by camera-space depth (smallest `clip_z` first,
/// i.e. closest to camera), with screen distance as the tiebreaker. This
/// prevents the snap from latching onto a back-face vert that projects
/// near the cursor but is occluded by the front face. The
/// `world_to_viewport_with_depth` call returns a `Vec3` whose z is the
/// world-space distance from the camera near plane along the view axis.
///
/// The returned snap's `face_idx` prefers `hover_face` when that face
/// uses the vert; otherwise it falls back to whichever face the vert
/// was first found on. Picking a face the cursor is actually over keeps
/// the commit pipeline's per-face grouping consistent with what the
/// user sees.
fn compute_vert_snap_all(
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    hover_face: Option<usize>,
) -> Option<KnifeSnapTarget> {
    let tolerance =
        window_pixels_to_target_pixels(KNIFE_SNAP_PIXELS, camera, viewport_entity, viewport_query);

    // best: (vert_idx, clip_z, screen_dist, face_idx)
    let mut best: Option<(usize, f32, f32, usize)> = None;
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
        for &vi in polygon {
            if !seen.insert(vi) {
                continue;
            }
            let local = cache.vertices[vi];
            let world = brush_global.transform_point(local);
            let Ok(screen_with_depth) = camera.world_to_viewport_with_depth(cam_tf, world) else {
                continue;
            };
            let screen = screen_with_depth.truncate();
            let clip_z = screen_with_depth.z;
            let dist = (screen - viewport_cursor).length();
            if dist > tolerance {
                continue;
            }
            let assigned_face = match hover_face {
                Some(hf) if cache.face_polygons[hf].contains(&vi) => hf,
                _ => face_idx,
            };
            // Ordering: prefer smaller clip_z (closer to camera); on ties
            // (within DEPTH_EPS), prefer smaller screen distance.
            let take = match best {
                None => true,
                Some((_, prev_z, prev_dist, _)) => {
                    if (clip_z - prev_z).abs() <= DEPTH_TIE_EPSILON {
                        dist < prev_dist
                    } else {
                        clip_z < prev_z
                    }
                }
            };
            if take {
                best = Some((vi, clip_z, dist, assigned_face));
            }
        }
    }
    let (vi, _, _, face_idx) = best?;
    let local = cache.vertices[vi];
    let world = brush_global.transform_point(local);
    Some(KnifeSnapTarget {
        world_pos: world,
        local_pos: local,
        kind: KnifeSnapKind::Vertex,
        face_idx,
        edge_pair: None,
        vert_idx: Some(vi),
        path_point_idx: None,
    })
}

/// Scan every face polygon's edges (deduplicated by canonical
/// `(min, max)` pair). Returns the best edge-midpoint snap (only when
/// `shift` is held) and the best edge-point snap. Edges shared between
/// faces are processed exactly once, so the snap target stays put when
/// the cursor crosses a face boundary in screen space.
///
/// Candidates are ordered by camera-space depth at the snap point
/// (smallest `clip_z` first, i.e. closest to camera), with screen
/// distance as the tiebreaker. Without the depth check, an edge on the
/// back of the brush whose projection falls within tolerance of the
/// cursor could outrank a closer edge on the front face the user is
/// pointing at, and the commit would attribute the click to a back face.
///
/// The returned snap's `face_idx` prefers `hover_face` when that face
/// uses the edge; otherwise the first face the edge was found on.
fn compute_edge_snap_all(
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    hover_face: Option<usize>,
    shift: bool,
) -> (Option<KnifeSnapTarget>, Option<KnifeSnapTarget>) {
    let tolerance =
        window_pixels_to_target_pixels(KNIFE_SNAP_PIXELS, camera, viewport_entity, viewport_query);

    // Best edge-point: (a_vi, b_vi, t, clip_z, dist, face_idx)
    let mut best_edge_point: Option<(usize, usize, f32, f32, f32, usize)> = None;
    // Best edge-midpoint: (a_vi, b_vi, clip_z, dist, face_idx)
    let mut best_midpoint: Option<(usize, usize, f32, f32, usize)> = None;
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();

    for (face_idx, polygon) in cache.face_polygons.iter().enumerate() {
        let n = polygon.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a_vi = polygon[i];
            let b_vi = polygon[(i + 1) % n];
            let canon = canonical_edge(a_vi, b_vi);
            if !seen.insert(canon) {
                continue;
            }
            let a_local = cache.vertices[a_vi];
            let b_local = cache.vertices[b_vi];
            let a_world = brush_global.transform_point(a_local);
            let b_world = brush_global.transform_point(b_local);
            let Ok(a_screen3) = camera.world_to_viewport_with_depth(cam_tf, a_world) else {
                continue;
            };
            let Ok(b_screen3) = camera.world_to_viewport_with_depth(cam_tf, b_world) else {
                continue;
            };
            let a_screen = a_screen3.truncate();
            let b_screen = b_screen3.truncate();

            let ab = b_screen - a_screen;
            let len_sq = ab.length_squared();
            let t = if len_sq > 1e-6 {
                ((viewport_cursor - a_screen).dot(ab) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let closest_screen = a_screen + ab * t;
            let dist = (closest_screen - viewport_cursor).length();
            // Linearly interpolate clip-space depth between the two edge
            // endpoints. This is approximate (true projection isn't
            // linear in screen space) but good enough to discriminate
            // front-face from back-face edges on a typical brush.
            let snap_clip_z = a_screen3.z + (b_screen3.z - a_screen3.z) * t;
            // Pick the face_idx attached to this edge snap: prefer
            // `hover_face` if it owns the edge so cross-face cuts get
            // the right side.
            let assigned_face = match hover_face {
                Some(hf) if face_polygon_has_edge(&cache.face_polygons[hf], a_vi, b_vi) => hf,
                _ => face_idx,
            };
            // Note: `dist <= tolerance` is a NON-STRICT compare so a
            // cursor sitting exactly on the edge in screen space
            // (distance == 0) still snaps; never reject zero-distance
            // matches.
            if dist <= tolerance {
                let take = match best_edge_point {
                    None => true,
                    Some((_, _, _, prev_z, prev_dist, _)) => {
                        if (snap_clip_z - prev_z).abs() <= DEPTH_TIE_EPSILON {
                            dist < prev_dist
                        } else {
                            snap_clip_z < prev_z
                        }
                    }
                };
                if take {
                    best_edge_point = Some((a_vi, b_vi, t, snap_clip_z, dist, assigned_face));
                }
            }

            if shift {
                let mid_screen = a_screen.lerp(b_screen, 0.5);
                let mid_dist = (mid_screen - viewport_cursor).length();
                let mid_clip_z = (a_screen3.z + b_screen3.z) * 0.5;
                if mid_dist <= tolerance {
                    let take = match best_midpoint {
                        None => true,
                        Some((_, _, prev_z, prev_dist, _)) => {
                            if (mid_clip_z - prev_z).abs() <= DEPTH_TIE_EPSILON {
                                mid_dist < prev_dist
                            } else {
                                mid_clip_z < prev_z
                            }
                        }
                    };
                    if take {
                        best_midpoint = Some((a_vi, b_vi, mid_clip_z, mid_dist, assigned_face));
                    }
                }
            }
        }
    }

    let midpoint_snap = best_midpoint.map(|(a_vi, b_vi, _, _, face_idx)| {
        let a_local = cache.vertices[a_vi];
        let b_local = cache.vertices[b_vi];
        let mid_local = a_local.lerp(b_local, 0.5);
        let mid_world = brush_global.transform_point(mid_local);
        KnifeSnapTarget {
            world_pos: mid_world,
            local_pos: mid_local,
            kind: KnifeSnapKind::EdgeMidpoint,
            face_idx,
            edge_pair: Some(canonical_edge(a_vi, b_vi)),
            vert_idx: None,
            path_point_idx: None,
        }
    });

    let point_snap = best_edge_point.map(|(a_vi, b_vi, t, _, _, face_idx)| {
        let a_local = cache.vertices[a_vi];
        let b_local = cache.vertices[b_vi];
        let snap_local = a_local.lerp(b_local, t);
        let snap_world = brush_global.transform_point(snap_local);
        KnifeSnapTarget {
            world_pos: snap_world,
            local_pos: snap_local,
            kind: KnifeSnapKind::EdgePoint,
            face_idx,
            edge_pair: Some(canonical_edge(a_vi, b_vi)),
            vert_idx: None,
            path_point_idx: None,
        }
    });

    (midpoint_snap, point_snap)
}

/// Compute a grid-point snap on `face_idx`'s plane. Projects the
/// cursor onto the face plane, then rounds the projected point's
/// brush-local coordinates to the nearest multiple of `grid_size` along
/// the two axes most-aligned with the face plane.
///
/// Returns `None` if:
///   - The ray-cursor projection misses (parallel ray, hit behind cam),
///   - The rounded point is too far from the cursor in screen space
///     (outside `KNIFE_SNAP_PIXELS`),
///   - The rounded point lies outside the face's screen polygon.
fn compute_grid_snap(
    face_idx: usize,
    viewport_cursor: Vec2,
    cache: &BrushMeshCache,
    brush_global: &GlobalTransform,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
    grid_size: f32,
) -> Option<KnifeSnapTarget> {
    if grid_size <= 0.0 {
        return None;
    }
    let polygon = cache.face_polygons.get(face_idx)?;
    if polygon.len() < 3 {
        return None;
    }
    let ring_local: Vec<Vec3> = polygon.iter().map(|&vi| cache.vertices[vi]).collect();
    let local_normal = jackdaw_geometry::newell_normal(&ring_local);
    if local_normal.length_squared() < 1e-12 {
        return None;
    }

    // World hit on the face plane via the cursor ray.
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

    // Snap the WORLD position to the grid: the grid lives in world
    // space (it's a global setting; the visible InfiniteGrid is in
    // world coords), so we round world.xyz to multiples of grid_size,
    // then project the rounded point back to the face plane so the
    // result is exactly on the plane.
    let snap_world_raw = Vec3::new(
        (hit_world.x / grid_size).round() * grid_size,
        (hit_world.y / grid_size).round() * grid_size,
        (hit_world.z / grid_size).round() * grid_size,
    );
    // Project onto plane: snap_world_raw + n * (centroid - snap_world_raw) dot n
    let to_centroid = centroid_world - snap_world_raw;
    let along = to_centroid.dot(world_normal);
    let snap_world = snap_world_raw + world_normal * along;

    // Tolerance check: rounded grid point must be within snap range in
    // screen space, otherwise the user is clearly aiming somewhere else.
    let tolerance =
        window_pixels_to_target_pixels(KNIFE_SNAP_PIXELS, camera, viewport_entity, viewport_query);
    let Ok(snap_screen) = camera.world_to_viewport(cam_tf, snap_world) else {
        return None;
    };
    let dist = (snap_screen - viewport_cursor).length();
    if dist > tolerance {
        return None;
    }

    // Reject grid points that land outside the face screen polygon.
    // Otherwise the snap dot could appear off-face when the cursor is
    // near a face edge.
    let screen_verts: Vec<Vec2> = polygon
        .iter()
        .filter_map(|&vi| {
            let world = brush_global.transform_point(cache.vertices[vi]);
            camera.world_to_viewport(cam_tf, world).ok()
        })
        .collect();
    if screen_verts.len() != polygon.len() || !point_in_polygon_2d(snap_screen, &screen_verts) {
        return None;
    }

    // Brush-local snap position.
    let affine = brush_global.affine();
    let inv = affine.inverse();
    let local_pos = inv.transform_point3(snap_world);

    Some(KnifeSnapTarget {
        world_pos: snap_world,
        local_pos,
        kind: KnifeSnapKind::GridPoint,
        face_idx,
        edge_pair: None,
        vert_idx: None,
        path_point_idx: None,
    })
}

/// Returns `true` if the polygon (vert indices into `cache.vertices`)
/// contains the edge `(a_vi, b_vi)` as a consecutive pair (either
/// orientation).
fn face_polygon_has_edge(polygon: &[usize], a_vi: usize, b_vi: usize) -> bool {
    let n = polygon.len();
    for i in 0..n {
        let p = polygon[i];
        let q = polygon[(i + 1) % n];
        if (p == a_vi && q == b_vi) || (p == b_vi && q == a_vi) {
            return true;
        }
    }
    false
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

/// Run the queued path as a single knife commit, using only topology
/// mutations on the `HalfedgeMesh` (no CDT). Invoked from the
/// `brush.knife.commit` operator's queued closure so the framework
/// captures the cut in a single snapshot-based undo entry.
///
/// Pipeline:
///
/// 1. **Snapshot** `start_brush` and the current `HalfedgeMesh` so we can
///    roll back atomically if anything panics or fails catastrophically.
/// 2. **First, resolve each path point to a live `VertKey`**.
///    Vertex snaps lookup by position; edge snaps `split_edge` the live
///    sub-edge containing the click; interior snaps `face_poke` the live
///    face the click lies in. Path-point reclicks inherit the prior
///    resolved vert.
/// 3. **Then chord each consecutive pair**. If both verts share a
///    live face's ring, `split_face` directly. Otherwise route across
///    one or more adjacent faces via intermediate `split_edge` /
///    `split_face` calls.
/// 4. **Finally flatten + sync `Brush`**. Sub-faces inherit the parent
///    face's `material_idx`, so per-slot `BrushFaceData` (UV axes,
///    material handle, etc.) is copied from `start_brush` keyed by
///    `material_idx`.
//
// Takes `&mut World` rather than a `Query` triple because the operator
// framework needs to drive the cut as a queued closure: the brush
// snapshot, the halfedge mutations, and the final brush sync each touch
// different components in sequence, and exclusive world access lets us
// drop one mutable borrow before grabbing the next without smuggling
// queries through the operator system signature.
fn commit_path(world: &mut World, brush_entity: Entity) {
    let Some(start_brush) = world.get::<Brush>(brush_entity).cloned() else {
        world.resource_mut::<KnifeMode>().path.clear();
        return;
    };
    let Some(halfedge_ref) = world.get::<BrushHalfedge>(brush_entity) else {
        world.resource_mut::<KnifeMode>().path.clear();
        return;
    };
    // Snapshot the live HalfedgeMesh so a catastrophe in either resolve
    // or chord (panic-equivalent return path) restores it exactly.
    let start_mesh = halfedge_ref.mesh.clone();
    let start_vert_keys = halfedge_ref.vert_keys.clone();
    let start_face_keys = halfedge_ref.face_keys.clone();

    // --- First, resolve each path point to a live VertKey. -------------
    //
    // Walk the user's path in order. For each click, depending on the
    // snap kind, perform exactly one topology mutation (or none if the
    // click resolved to an existing vert) and record the resulting
    // VertKey. PathPoint reclicks inherit the prior resolved vert.
    let path_points: Vec<KnifePathPoint> = world.resource::<KnifeMode>().path.clone();
    if path_points.len() < 2 {
        world.resource_mut::<KnifeMode>().path.clear();
        return;
    }
    let mut path_verts: Vec<VertKey> = Vec::with_capacity(path_points.len());
    let mut resolve_failed = false;
    {
        let Some(mut halfedge) = world.get_mut::<BrushHalfedge>(brush_entity) else {
            world.resource_mut::<KnifeMode>().path.clear();
            return;
        };
        for (i, point) in path_points.iter().enumerate() {
            // PathPoint reclicks: inherit the resolved VertKey from the
            // source click. This is "no new geometry" by construction.
            if let Some(src_idx) = point.source_path_idx
                && src_idx < i
                && let Some(&v) = path_verts.get(src_idx)
            {
                path_verts.push(v);
                continue;
            }
            match resolve_path_point(&mut halfedge.mesh, point, &start_brush) {
                Ok(v) => path_verts.push(v),
                Err(reason) => {
                    warn!(
                        "Knife: path point {} resolve failed ({}); aborting commit",
                        i, reason
                    );
                    resolve_failed = true;
                    break;
                }
            }
        }
    }

    if resolve_failed {
        // Restore the snapshot and bail out without pushing to history.
        if let Some(mut halfedge) = world.get_mut::<BrushHalfedge>(brush_entity) {
            halfedge.mesh = start_mesh;
            halfedge.vert_keys = start_vert_keys;
            halfedge.face_keys = start_face_keys;
        }
        world.resource_mut::<KnifeMode>().path.clear();
        return;
    }

    // --- Then chord each consecutive pair. ------------------------------
    //
    // For each `(path_verts[i], path_verts[i+1])`:
    //   - Same live face containing both: `split_face`.
    //   - Otherwise: cross-face routing via `split_edge` then `split_face`,
    //     recursing until both endpoints share a face.
    //
    // A segment that can't be resolved (no path, adjacent verts that
    // would yield a degenerate face, or collapsed endpoints) is logged
    // and skipped but doesn't abort the commit.
    let mut applied_segments = 0usize;
    {
        let Some(mut halfedge) = world.get_mut::<BrushHalfedge>(brush_entity) else {
            world.resource_mut::<KnifeMode>().path.clear();
            return;
        };
        for i in 0..path_verts.len().saturating_sub(1) {
            let va = path_verts[i];
            let vb = path_verts[i + 1];
            if va == vb {
                debug!(
                    "Knife: segment {} -> {} collapsed (same vert); skipping",
                    i,
                    i + 1
                );
                continue;
            }
            match chord_between_verts(&mut halfedge.mesh, va, vb) {
                Ok(applied) => {
                    if applied {
                        applied_segments += 1;
                    } else {
                        debug!(
                            "Knife: segment {} -> {} no-op (chord already exists)",
                            i,
                            i + 1
                        );
                    }
                }
                Err(reason) => warn!(
                    "Knife: segment {} -> {} chord failed: {}; skipping",
                    i,
                    i + 1,
                    reason
                ),
            }
        }
    }

    // The resolve pass may have applied mutations (edge splits, face
    // pokes) even if every chord segment was a no-op. We still want to
    // commit those if any path point introduced new geometry.
    // Detect "did anything change" by comparing mesh sizes; if not,
    // restore and bail.
    let (mesh_unchanged, slot_to_src_material, new_topology) = {
        let Some(halfedge) = world.get::<BrushHalfedge>(brush_entity) else {
            world.resource_mut::<KnifeMode>().path.clear();
            return;
        };
        let mesh_unchanged = halfedge.mesh.vert_count() == start_mesh.vert_count()
            && halfedge.mesh.edge_count() == start_mesh.edge_count()
            && halfedge.mesh.face_count() == start_mesh.face_count();
        let mut slot_to_src_material: Vec<u32> = halfedge
            .mesh
            .faces
            .iter()
            .map(|(_, f)| f.material_idx)
            .collect();
        slot_to_src_material.sort();
        let new_topology = halfedge.mesh.flatten_to_topology();
        (mesh_unchanged, slot_to_src_material, new_topology)
    };
    if mesh_unchanged && applied_segments == 0 {
        debug!("Knife: commit applied no changes; restoring snapshot");
        if let Some(mut halfedge) = world.get_mut::<BrushHalfedge>(brush_entity) {
            halfedge.mesh = start_mesh;
            halfedge.vert_keys = start_vert_keys;
            halfedge.face_keys = start_face_keys;
        }
        world.resource_mut::<KnifeMode>().path.clear();
        return;
    }

    // --- Finally flatten + sync brush. ----------------------------------
    //
    // Every sub-face produced by `split_edge` / `split_face` / `face_poke`
    // inherits its parent's `material_idx`. `flatten_to_topology` sorts
    // faces by `material_idx`, so sub-faces appear contiguously at the
    // original slot. Rebuild `brush.faces` per output polygon, copying
    // from `start_brush.faces[material_idx]`. UV axes copy verbatim:
    // sub-faces are coplanar with their parent, so the parent's tangent
    // basis is correct for every sub-face.
    let mut new_faces: Vec<jackdaw_geometry::BrushFaceData> =
        Vec::with_capacity(new_topology.polygons.len());
    for &src_material in &slot_to_src_material {
        let src_idx = src_material as usize;
        let src = start_brush
            .faces
            .get(src_idx)
            .cloned()
            .or_else(|| start_brush.faces.last().cloned())
            .unwrap_or_default();
        new_faces.push(src);
    }

    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (idx, face_data) in new_faces.iter_mut().enumerate() {
        if idx < new_topology.polygons.len() {
            let normal = new_topology.face_normal_with(&positions, idx);
            let v0_idx =
                new_topology.loops[new_topology.polygons[idx].loop_start as usize].vert as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
        }
    }

    let new_brush = {
        let Some(mut brush_mut) = world.get_mut::<Brush>(brush_entity) else {
            // Defensive: if the Brush component vanished mid-commit (should
            // not happen during normal use), just clear the path.
            world.resource_mut::<KnifeMode>().path.clear();
            return;
        };
        brush_mut.faces = new_faces;
        brush_mut.topology = new_topology;
        brush_mut.clone()
    };

    // Re-lift HalfedgeMesh so vert_keys / face_keys stay consistent with
    // the flattened brush topology.
    let new_mesh = HalfedgeMesh::lift_from_topology(&new_brush.topology);
    let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
    let mut new_face_keys: Vec<FaceKey> = vec![FaceKey::default(); new_mesh.faces.len()];
    for (k, f) in new_mesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    if let Some(mut halfedge) = world.get_mut::<BrushHalfedge>(brush_entity) {
        halfedge.mesh = new_mesh;
        halfedge.vert_keys = new_vert_keys;
        halfedge.face_keys = new_face_keys;
    }

    // Push the brush state through to the scene AST so subsequent prefab
    // reloads see the cut. Previously the `SetBrush::execute` path did
    // this via `apply_brush`; the operator-driven path lacks that hook,
    // so the sync has to happen explicitly before the framework captures
    // the after-snapshot.
    crate::brush::sync_brush_to_ast(world, brush_entity, &new_brush);

    world.resource_mut::<KnifeMode>().path.clear();
}

/// Plane / position tolerance used when matching a click position
/// against live mesh elements. Brush-local coordinates are typically
/// in the [0, 1024] range for level geometry, and edge / poke positions
/// from the snap pipeline land within a fraction of a world unit of the
/// true edge/face, so 1e-4 is a safe match radius.
const KNIFE_POSITION_EPSILON: f32 = 1e-4;

/// Resolve a single path point to a live `VertKey` in `mesh`. May
/// mutate the mesh (`split_edge` / `face_poke`).
fn resolve_path_point(
    mesh: &mut HalfedgeMesh,
    point: &KnifePathPoint,
    start_brush: &Brush,
) -> Result<VertKey, &'static str> {
    match point.kind {
        KnifeSnapKind::Vertex => resolve_vertex_snap(mesh, point, start_brush),
        KnifeSnapKind::EdgePoint | KnifeSnapKind::EdgeMidpoint => {
            resolve_edge_snap(mesh, point, start_brush)
        }
        KnifeSnapKind::FaceInterior | KnifeSnapKind::GridPoint => {
            resolve_interior_snap(mesh, point)
        }
        KnifeSnapKind::PathPoint => {
            // PathPoint reclicks with no resolvable source (e.g., snap
            // metadata was incomplete) fall through to the underlying
            // kind based on whatever snap data was copied.
            if point.vert_idx.is_some() {
                resolve_vertex_snap(mesh, point, start_brush)
            } else if point.edge_pair.is_some() {
                resolve_edge_snap(mesh, point, start_brush)
            } else {
                resolve_interior_snap(mesh, point)
            }
        }
    }
}

/// Vertex snap: look up the live `VertKey` by position against the live
/// mesh. The recorded `vert_idx` is the index in `start_brush.topology`
/// at modal-start time; we map that to a 3D position and find any vert
/// matching it within `KNIFE_POSITION_EPSILON`.
fn resolve_vertex_snap(
    mesh: &HalfedgeMesh,
    point: &KnifePathPoint,
    start_brush: &Brush,
) -> Result<VertKey, &'static str> {
    let vi = point.vert_idx.ok_or("vertex snap missing vert_idx")?;
    let target = start_brush
        .topology
        .vertices
        .get(vi)
        .map(|v| v.position)
        .ok_or("vertex snap vert_idx out of range")?;
    find_vert_by_position(mesh, target).ok_or("vertex snap vert not found in live mesh")
}

/// Edge snap: find a live edge whose endpoints flank the click position
/// (within `KNIFE_POSITION_EPSILON` in 3D), then `split_edge` it. This
/// finds the right sub-edge even after earlier mutations split the
/// original edge into pieces.
fn resolve_edge_snap(
    mesh: &mut HalfedgeMesh,
    point: &KnifePathPoint,
    _start_brush: &Brush,
) -> Result<VertKey, &'static str> {
    let click = point.local_pos;
    let Some((ek, t)) = find_live_edge_for_position(mesh, click) else {
        // Fallback: if a sibling vert already exists at this position
        // from an earlier mutation (e.g., this same click position was
        // reached by an earlier face_poke / split_edge), return that
        // vert directly. This handles the "two consecutive edge clicks
        // on the same original edge" case where the second click sits
        // exactly on the new vert introduced by the first.
        return find_vert_by_position(mesh, click)
            .ok_or("edge snap: no live edge or vert contains click position");
    };
    split_edge(mesh, ek, t).map_err(|_| "split_edge failed")
}

/// Interior snap: find the live face whose ring contains the click
/// position (point-in-polygon in the face's plane), then `face_poke`.
/// After earlier mutations the click may lie inside a sub-face (fan
/// tri or split-face child); the search walks every face and returns
/// the one matching.
fn resolve_interior_snap(
    mesh: &mut HalfedgeMesh,
    point: &KnifePathPoint,
) -> Result<VertKey, &'static str> {
    let click = point.local_pos;
    // First: maybe the click coincides with a live vert already (the
    // user clicked exactly on a newly-introduced center vert from a
    // prior poke). Reuse it without creating new geometry.
    if let Some(v) = find_vert_by_position(mesh, click) {
        return Ok(v);
    }
    let face_key = find_live_face_containing_point(mesh, click)
        .ok_or("interior snap: no live face contains click position")?;
    // Project the click onto the face plane for robust face_poke; the
    // op enforces a plane-distance tolerance and rejects out-of-plane
    // points.
    let projected = project_point_onto_face(mesh, face_key, click);
    let result = face_poke(mesh, face_key, projected).map_err(|_| "face_poke failed")?;
    Ok(result.center_vert)
}

/// Walk every edge in `mesh`. Return any edge whose segment passes
/// within `KNIFE_POSITION_EPSILON` of `click`, plus the parameter `t`
/// along that edge at the closest point. We strongly prefer interior
/// hits (0 < t < 1) over endpoint hits: an endpoint hit means the
/// click coincides with a vert, which the caller handles separately.
fn find_live_edge_for_position(mesh: &HalfedgeMesh, click: Vec3) -> Option<(EdgeKey, f32)> {
    let mut best: Option<(EdgeKey, f32, f32)> = None; // (edge, t, dist)
    for (k, e) in mesh.edges.iter() {
        let p0 = mesh.verts[e.v[0]].co;
        let p1 = mesh.verts[e.v[1]].co;
        let dir = p1 - p0;
        let len_sq = dir.length_squared();
        if len_sq < 1e-12 {
            continue;
        }
        let t = ((click - p0).dot(dir) / len_sq).clamp(0.0, 1.0);
        let closest = p0 + dir * t;
        let dist = (closest - click).length();
        if dist > KNIFE_POSITION_EPSILON {
            continue;
        }
        // Reject hits at endpoints (within a small tolerance in t):
        // those mean the click coincides with a vert, which the caller
        // resolves via vert reuse rather than split_edge.
        if !(1e-4..=1.0 - 1e-4).contains(&t) {
            continue;
        }
        match best {
            Some((_, _, prev_dist)) if prev_dist <= dist => {}
            _ => best = Some((k, t, dist)),
        }
    }
    best.map(|(k, t, _)| (k, t))
}

/// Walk every face in `mesh`. Return any face whose 2D ring (projected
/// onto its newell plane) contains `click` AND whose plane is within
/// `KNIFE_POSITION_EPSILON * 100` of `click`. We use a looser plane
/// tolerance here (1e-2) because subsequent `face_poke` projects the
/// click back onto the plane anyway.
fn find_live_face_containing_point(mesh: &HalfedgeMesh, click: Vec3) -> Option<FaceKey> {
    for (k, face) in mesh.faces.iter() {
        let ring_positions = face_ring_positions(mesh, k);
        if ring_positions.len() < 3 {
            continue;
        }
        // Distance to face plane.
        let n = face.normal_cache;
        if n.length_squared() < 1e-12 {
            continue;
        }
        let centroid: Vec3 =
            ring_positions.iter().copied().sum::<Vec3>() / ring_positions.len() as f32;
        let plane_dist = n.dot(click - centroid).abs();
        if plane_dist > 1e-2 {
            continue;
        }
        if point_in_polygon_3d(click, &ring_positions, n) {
            return Some(k);
        }
    }
    None
}

/// Project `click` onto `face`'s plane (the face's `normal_cache` + a
/// ring-centroid anchor). Used to keep `face_poke`'s plane-tolerance
/// check happy.
fn project_point_onto_face(mesh: &HalfedgeMesh, face: FaceKey, click: Vec3) -> Vec3 {
    let ring = face_ring_positions(mesh, face);
    if ring.is_empty() {
        return click;
    }
    let centroid: Vec3 = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
    let n = mesh.faces[face].normal_cache;
    if n.length_squared() < 1e-12 {
        return click;
    }
    let signed = n.dot(click - centroid);
    click - n * signed
}

/// Return the world (brush-local) ring positions of `face` in loop
/// order.
fn face_ring_positions(mesh: &HalfedgeMesh, face: FaceKey) -> Vec<Vec3> {
    let f = &mesh.faces[face];
    let mut out = Vec::with_capacity(f.loop_count as usize);
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        out.push(mesh.verts[mesh.loops[cur].vert].co);
        cur = mesh.loops[cur].next;
    }
    out
}

/// 3D point-in-polygon: project the polygon and the test point onto
/// the plane spanned by the polygon (using `normal` as the plane
/// normal), then run a 2D ray-cast. Returns true when `point` is
/// inside the projected polygon.
fn point_in_polygon_3d(point: Vec3, ring: &[Vec3], normal: Vec3) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return false;
    }
    // Build a 2D basis perpendicular to `n`.
    let u_seed = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = (u_seed - n * u_seed.dot(n)).normalize_or_zero();
    if u == Vec3::ZERO {
        return false;
    }
    let v = n.cross(u);
    let origin = ring[0];
    let to_2d = |p: Vec3| -> Vec2 {
        let d = p - origin;
        Vec2::new(d.dot(u), d.dot(v))
    };
    let ring_2d: Vec<Vec2> = ring.iter().map(|&p| to_2d(p)).collect();
    let point_2d = to_2d(point);
    // Standard ray-cast.
    let n2d = ring_2d.len();
    let mut inside = false;
    let mut j = n2d - 1;
    for i in 0..n2d {
        let pi = ring_2d[i];
        let pj = ring_2d[j];
        if ((pi.y > point_2d.y) != (pj.y > point_2d.y))
            && (point_2d.x < (pj.x - pi.x) * (point_2d.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Insert a chord from `va` to `vb`. If the two verts share a live
/// face's ring, `split_face` directly. Otherwise recursively split
/// across one or more adjacent faces.
///
/// Returns `Ok(true)` if at least one `split_face` mutation was applied,
/// `Ok(false)` if the chord was a no-op (e.g., the two verts are
/// already connected by a single edge or have collapsed to the same
/// face boundary). `Err` for unrecoverable cases.
fn chord_between_verts(
    mesh: &mut HalfedgeMesh,
    va: VertKey,
    vb: VertKey,
) -> Result<bool, &'static str> {
    chord_between_verts_recursive(mesh, va, vb, 0)
}

/// Recursion depth limit: cuts that cross more than this many faces
/// are extremely unlikely on real geometry and indicate a runaway loop.
const KNIFE_CROSS_FACE_MAX_DEPTH: u32 = 16;

fn chord_between_verts_recursive(
    mesh: &mut HalfedgeMesh,
    va: VertKey,
    vb: VertKey,
    depth: u32,
) -> Result<bool, &'static str> {
    if depth >= KNIFE_CROSS_FACE_MAX_DEPTH {
        return Err("cross-face routing exceeded depth limit");
    }
    if va == vb {
        return Ok(false);
    }
    // Same face containing both verts: direct split_face.
    if let Some(face) = find_face_containing_both_verts(mesh, va, vb) {
        // If `va` and `vb` are adjacent in the face's ring, a chord
        // would yield a degenerate face. The path already follows the
        // existing edge between them; nothing to do.
        if are_ring_neighbors(mesh, face, va, vb) {
            return Ok(false);
        }
        split_face(mesh, face, va, vb).map_err(|_| "split_face failed")?;
        return Ok(true);
    }

    // Cross-face: find a face adjacent to `va` whose boundary the
    // segment (va_pos -> vb_pos) crosses. Split that boundary edge,
    // and recurse with the intermediate vert as the new `va`.
    let va_pos = mesh.verts[va].co;
    let vb_pos = mesh.verts[vb].co;

    let Some((boundary_face, crossing_edge, t)) =
        find_outgoing_face_and_edge(mesh, va, va_pos, vb_pos)
    else {
        return Err("no outgoing face/edge for cross-face chord");
    };

    // Split the boundary edge at the crossing.
    let inter = split_edge(mesh, crossing_edge, t).map_err(|_| "split_edge failed")?;

    // Chord va -> inter inside `boundary_face`. The face was just split
    // by split_edge such that its ring includes the new vert; if va is
    // also on that face's ring (it must be by construction), we can
    // split_face directly UNLESS va and inter are adjacent (the chord
    // would be degenerate, which means va is actually the endpoint of
    // the crossing edge we just split, in which case the chord is just
    // the edge itself and there's nothing to split).
    let live_face = find_face_containing_both_verts(mesh, va, inter);
    let mut any = false;
    if let Some(face) = live_face {
        if !are_ring_neighbors(mesh, face, va, inter) {
            split_face(mesh, face, va, inter).map_err(|_| "split_face failed (cross-face leg)")?;
            any = true;
        }
        let _ = boundary_face; // captured for diagnostics only
    } else {
        // va isn't on the same face as `inter` post-split. This can
        // happen when `va` was at a fan vertex from a prior face_poke
        // and `boundary_face` was actually a different face. Recurse
        // from `va` toward `inter` so the next pass finds a route.
        let sub = chord_between_verts_recursive(mesh, va, inter, depth + 1)?;
        if sub {
            any = true;
        }
    }

    // Recurse for the second leg: inter -> vb.
    let sub = chord_between_verts_recursive(mesh, inter, vb, depth + 1)?;
    Ok(any || sub)
}

/// Walk every face. Return any whose ring contains both `va` and `vb`.
/// Deterministic for a given mesh state.
fn find_face_containing_both_verts(
    mesh: &HalfedgeMesh,
    va: VertKey,
    vb: VertKey,
) -> Option<FaceKey> {
    for (k, f) in mesh.faces.iter() {
        let mut has_a = false;
        let mut has_b = false;
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            let v = mesh.loops[cur].vert;
            if v == va {
                has_a = true;
            }
            if v == vb {
                has_b = true;
            }
            cur = mesh.loops[cur].next;
        }
        if has_a && has_b {
            return Some(k);
        }
    }
    None
}

/// Returns true if `va` and `vb` are consecutive in `face`'s ring
/// (either direction). `split_face` errors on adjacent verts; this is
/// the gate before we try.
fn are_ring_neighbors(mesh: &HalfedgeMesh, face: FaceKey, va: VertKey, vb: VertKey) -> bool {
    let f = &mesh.faces[face];
    let n = f.loop_count as usize;
    if n < 2 {
        return false;
    }
    let mut cur = f.loop_first;
    let mut ring: Vec<VertKey> = Vec::with_capacity(n);
    for _ in 0..n {
        ring.push(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        if (p == va && q == vb) || (p == vb && q == va) {
            return true;
        }
    }
    false
}

/// Find an outgoing face for the cross-face chord. Walks every face
/// incident to `va` (faces whose ring contains `va`); for each, finds
/// any edge OF THE FACE that the segment `va_pos -> vb_pos` crosses in
/// the face's plane, excluding edges containing `va` itself. Returns
/// `(face, edge, t)` where `t` is the parameter along the edge.
///
/// Multi-face traversal: the recursion in `chord_between_verts_recursive`
/// keeps calling this with progressively-updated endpoints, so a single
/// call only finds the FIRST face boundary crossing; subsequent recursive
/// calls find later crossings until both endpoints land on the same face.
fn find_outgoing_face_and_edge(
    mesh: &HalfedgeMesh,
    va: VertKey,
    va_pos: Vec3,
    vb_pos: Vec3,
) -> Option<(FaceKey, EdgeKey, f32)> {
    let mut best: Option<(FaceKey, EdgeKey, f32, f32)> = None; // face, edge, t, segment_t
    for (face_key, f) in mesh.faces.iter() {
        // face must contain `va`
        let mut ring: Vec<(LoopKey, VertKey)> = Vec::with_capacity(f.loop_count as usize);
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            ring.push((cur, mesh.loops[cur].vert));
            cur = mesh.loops[cur].next;
        }
        if !ring.iter().any(|&(_, v)| v == va) {
            continue;
        }
        // Walk each ring edge; skip edges touching va.
        for &(lp, _) in &ring {
            let edge_key = mesh.loops[lp].edge;
            let e = &mesh.edges[edge_key];
            if e.v[0] == va || e.v[1] == va {
                continue;
            }
            let e0 = mesh.verts[e.v[0]].co;
            let e1 = mesh.verts[e.v[1]].co;
            // Compute the 2D crossing of (va_pos -> vb_pos) with the
            // edge, in the face's plane.
            let n = f.normal_cache;
            let Some((edge_t, seg_t)) =
                segment_edge_intersection_in_plane(va_pos, vb_pos, e0, e1, n)
            else {
                continue;
            };
            // Both parameters must be strictly within (0, 1): the
            // segment must cross the edge interior, not just an
            // endpoint.
            if edge_t <= 1e-4 || edge_t >= 1.0 - 1e-4 {
                continue;
            }
            if seg_t <= 1e-4 || seg_t >= 1.0 + 1e-4 {
                // seg_t == 1 means the segment ends exactly at the
                // crossing; that's fine for terminal segments, accept.
                if seg_t < 1e-4 {
                    continue;
                }
            }
            // Prefer the smallest `seg_t` (first crossing along the
            // segment) so multi-face routing makes progress.
            let take = match best {
                None => true,
                Some((_, _, _, prev_seg)) => seg_t < prev_seg,
            };
            if take {
                best = Some((face_key, edge_key, edge_t, seg_t));
            }
        }
    }
    best.map(|(f, e, t, _)| (f, e, t))
}

/// Compute the intersection of two segments projected onto the plane
/// with normal `n`. Returns `(t_along_edge, t_along_segment)` where
/// both are in `[0, 1]` for interior crossings.
fn segment_edge_intersection_in_plane(
    p0: Vec3,
    p1: Vec3,
    e0: Vec3,
    e1: Vec3,
    n: Vec3,
) -> Option<(f32, f32)> {
    let n_norm = n.normalize_or_zero();
    if n_norm == Vec3::ZERO {
        return None;
    }
    let u_seed = if n_norm.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let u = (u_seed - n_norm * u_seed.dot(n_norm)).normalize_or_zero();
    if u == Vec3::ZERO {
        return None;
    }
    let v = n_norm.cross(u);
    let origin = e0;
    let to2 = |p: Vec3| -> Vec2 {
        let d = p - origin;
        Vec2::new(d.dot(u), d.dot(v))
    };
    let p0_2 = to2(p0);
    let p1_2 = to2(p1);
    let e0_2 = to2(e0);
    let e1_2 = to2(e1);
    let r = e1_2 - e0_2;
    let s = p1_2 - p0_2;
    let denom = r.x * s.y - r.y * s.x;
    if denom.abs() < 1e-9 {
        return None;
    }
    let q = p0_2 - e0_2;
    let t_edge = (q.x * s.y - q.y * s.x) / denom;
    let t_seg = (q.x * r.y - q.y * r.x) / denom;
    Some((t_edge, t_seg))
}

/// Find the vert with the smallest squared distance to `target`, but
/// only return it when within `KNIFE_POSITION_EPSILON`.
fn find_vert_by_position(mesh: &HalfedgeMesh, target: Vec3) -> Option<VertKey> {
    let eps_sq = KNIFE_POSITION_EPSILON * KNIFE_POSITION_EPSILON;
    let mut best: Option<(VertKey, f32)> = None;
    for (k, v) in mesh.verts.iter() {
        let d = (v.co - target).length_squared();
        match best {
            Some((_, prev)) if prev <= d => {}
            _ => best = Some((k, d)),
        }
    }
    let (k, d) = best?;
    if d <= eps_sq { Some(k) } else { None }
}
