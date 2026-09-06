use bevy::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DrawPhase {
    PlacingFirstCorner,
    DrawingFootprint,
    DrawingRotatedWidth,
    DrawingPolygon,
    ExtrudingDepth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum DrawMode {
    #[default]
    Add,
    Cut,
}

#[derive(Clone, Debug)]
pub(crate) struct DrawPlane {
    pub origin: Vec3,
    pub normal: Vec3,
    pub axis_u: Vec3,
    pub axis_v: Vec3,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveDraw {
    pub corner1: Vec3,
    pub corner2: Vec3,
    pub depth: f32,
    pub phase: DrawPhase,
    pub mode: DrawMode,
    pub plane: DrawPlane,
    pub extrude_start_cursor: Vec2,
    pub plane_locked: bool,
    /// World-space cursor position on the drawing plane (for crosshair preview).
    pub cursor_on_plane: Option<Vec3>,
    /// When set, the drawn shape will be CSG-unioned with this brush instead of spawning a new entity.
    pub append_target: Option<Entity>,
    /// True during press-drag-release rectangle drawing.
    pub drag_footprint: bool,
    /// Screen position at initial press (for drag vs click detection).
    pub press_screen_pos: Option<Vec2>,
    /// Placed polygon vertices in world space (polygon draw mode).
    pub polygon_vertices: Vec<Vec3>,
    /// Current cursor position on plane during polygon mode (for preview edge).
    pub polygon_cursor: Option<Vec3>,
    /// When true, constrain cursor to nearest 45-degree angle from last vertex.
    pub diagonal_snap: bool,
    /// Last successful face raycast hit point, for plane stickiness when raycast misses near edges.
    pub cached_face_hit: Option<Vec3>,
    /// Multi-viewport: camera + UI-node entities of the viewport this
    /// draw started in. Subsequent operators / per-frame updates
    /// route through these so the in-progress polygon stays bound to
    /// its origin viewport even if the cursor wanders elsewhere.
    pub camera: Option<Entity>,
    pub viewport: Option<Entity>,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct DrawBrushState {
    pub(crate) active: Option<ActiveDraw>,
}
