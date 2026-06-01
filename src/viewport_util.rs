use bevy::{picking::prelude::Pickable, prelude::*, ui::UiGlobalTransform};

use crate::default_style;
use crate::viewport::SceneViewport;

/// Window-to-camera-space mapping for the scene viewport node.
///
/// `top_left` is where the viewport node starts in window logical pixels,
/// `vp_size` is its logical size, and `remap` scales window-local pixels
/// onto the camera's render target (which may differ from the UI node
/// size on `HiDPI` / fractional scaling).
pub(crate) struct ViewportRemap {
    pub top_left: Vec2,
    pub vp_size: Vec2,
    pub remap: Vec2,
}

impl ViewportRemap {
    /// Compute remap parameters from the camera and the scene viewport's
    /// `ComputedNode` + `UiGlobalTransform`.
    pub fn new(camera: &Camera, computed: &ComputedNode, vp_transform: &UiGlobalTransform) -> Self {
        let scale = computed.inverse_scale_factor();
        let vp_pos = vp_transform.translation * scale;
        let vp_size = computed.size() * scale;
        let top_left = vp_pos - vp_size / 2.0;
        let target_size = camera.logical_viewport_size().unwrap_or(vp_size);
        Self {
            top_left,
            vp_size,
            remap: target_size / vp_size,
        }
    }
}

/// Build the rubber-band marquee overlay node spanning `start`..`current`
/// in window pixels. Returns the node bundle without a marker component so
/// each box-select system can attach its own marker and spawn-or-despawn
/// against its own state.
pub(crate) fn marquee_node(start: Vec2, current: Vec2) -> impl Bundle {
    let min = start.min(current);
    let max = start.max(current);
    let size = max - min;
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(min.x),
            top: Val::Px(min.y),
            width: Val::Px(size.x),
            height: Val::Px(size.y),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(default_style::SELECTION_MARQUEE_BG),
        BorderColor::all(default_style::SELECTION_MARQUEE_BORDER),
        GlobalZIndex(50),
        Pickable::IGNORE,
    )
}

/// Viewport-local `(min, max)` rectangle in render-target pixels for a
/// box-select that spanned `start`..`current` in window pixels. The caller
/// resolves the captured camera + viewport node and passes them in.
pub(crate) fn box_select_rect(
    camera: &Camera,
    vp_computed: &ComputedNode,
    vp_tf: &UiGlobalTransform,
    start: Vec2,
    current: Vec2,
) -> (Vec2, Vec2) {
    let map = ViewportRemap::new(camera, vp_computed, vp_tf);
    let start_local = start - map.top_left;
    let current_local = current - map.top_left;
    let min = start_local.min(current_local) * map.remap;
    let max = start_local.max(current_local) * map.remap;
    (min, max)
}

/// Convert a window cursor position to camera-space viewport coordinates,
/// remapping the cursor against a specific viewport UI-node entity rather
/// than assuming there's only one. Used by hover-routed systems that
/// already know which viewport the cursor is over (via `ActiveViewport`)
/// and by modal operators that captured a viewport at start.
///
/// The camera renders to an off-screen image whose logical size may differ
/// from the UI node's logical size (they diverge on `HiDPI` / fractional
/// scaling displays). This remaps UI-logical space into camera viewport
/// space so `camera.viewport_to_world()` / `camera.world_to_viewport()`
/// produce correct results.
pub(crate) fn window_to_viewport_cursor_for(
    cursor_pos: Vec2,
    camera: &Camera,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec2> {
    let Ok((computed, vp_transform)) = viewport_query.get(viewport_entity) else {
        return None;
    };
    remap_cursor(cursor_pos, camera, computed, vp_transform)
}

/// Like [`window_to_viewport_cursor_for`] but does not bounds-check
/// the cursor against the viewport rectangle. Returns `None` only if
/// `viewport_entity` is no longer a `SceneViewport`.
///
/// Used by modal operators that captured the viewport at drag-start
/// and need cursor coordinates to keep updating even when the user
/// drags past the viewport's edge into another panel. Bounds-checking
/// during a drag would force the operator to cancel mid-gesture and
/// snap the entity back to its start transform.
pub(crate) fn window_to_viewport_cursor_for_unbounded(
    cursor_pos: Vec2,
    camera: &Camera,
    viewport_entity: Entity,
    viewport_query: &Query<(&ComputedNode, &UiGlobalTransform), With<SceneViewport>>,
) -> Option<Vec2> {
    let Ok((computed, vp_transform)) = viewport_query.get(viewport_entity) else {
        return None;
    };
    let map = ViewportRemap::new(camera, computed, vp_transform);
    Some((cursor_pos - map.top_left) * map.remap)
}

fn remap_cursor(
    cursor_pos: Vec2,
    camera: &Camera,
    computed: &ComputedNode,
    vp_transform: &UiGlobalTransform,
) -> Option<Vec2> {
    let map = ViewportRemap::new(camera, computed, vp_transform);
    let local = cursor_pos - map.top_left;
    if local.x >= 0.0 && local.y >= 0.0 && local.x <= map.vp_size.x && local.y <= map.vp_size.y {
        Some(local * map.remap)
    } else {
        None
    }
}

/// Test whether a 2D point lies inside a convex or concave polygon (ray-casting algorithm).
pub(crate) fn point_in_polygon_2d(point: Vec2, polygon: &[Vec2]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi = polygon[i];
        let pj = polygon[j];
        if ((pi.y > point.y) != (pj.y > point.y))
            && (point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from a point to a line segment.
pub(crate) fn point_to_segment_dist(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let ap = point - a;
    let t = (ap.dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    let closest = a + ab * t;
    (point - closest).length()
}
