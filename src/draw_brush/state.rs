use crate::commands::{EditorCommand, deselect_entities, despawn_scene_entity};
use crate::scene_io::entity_by_scene_node_id;
use bevy::prelude::*;
use jackdaw_scene_types::{Brush, SceneNodeId};

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

/// Minimal data needed to respawn a brush entity.
#[derive(Clone)]
pub(crate) struct BrushData {
    pub(crate) node_id: SceneNodeId,
    pub(crate) brush: Brush,
    pub(crate) transform: Transform,
    pub(crate) name: String,
    pub(crate) parent_node_id: Option<SceneNodeId>,
}

fn scene_node_id_of(world: &mut World, entity: Entity) -> SceneNodeId {
    if let Some(id) = world.get::<SceneNodeId>(entity).copied() {
        return id;
    }
    let id = SceneNodeId::next();
    world.entity_mut(entity).insert(id);
    id
}

/// Read brush data from an existing entity. Mints a `SceneNodeId` if missing.
pub(crate) fn brush_data_from_entity(world: &mut World, entity: Entity) -> BrushData {
    let node_id = scene_node_id_of(world, entity);
    let parent = world.get::<ChildOf>(entity).map(|child_of| child_of.0);
    let parent_node_id = parent.map(|parent| scene_node_id_of(world, parent));

    BrushData {
        node_id,
        brush: world.get::<Brush>(entity).unwrap().clone(),
        transform: *world.get::<Transform>(entity).unwrap(),
        name: world
            .get::<Name>(entity)
            .map(std::string::ToString::to_string)
            .unwrap_or_default(),
        parent_node_id,
    }
}

/// Spawn a brush entity from stored data. Returns new entity ID.
pub(crate) fn spawn_brush_from_data(world: &mut World, data: &BrushData) -> Entity {
    let parent_entity = data
        .parent_node_id
        .and_then(|id| entity_by_scene_node_id(world, id));

    let mut ec = world.spawn((
        Name::new(data.name.clone()),
        data.brush.clone(),
        data.transform,
        data.node_id,
        Visibility::default(),
    ));
    if let Some(parent) = parent_entity {
        ec.insert(ChildOf(parent));
    }
    let entity = ec.id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Per-command undo entry for brush spawns from the legacy non-
/// operator paths (face extrude, brush clip/split). The draw-brush
/// modal operator doesn't push this; its `SnapshotDiff` covers the
/// whole transaction.
pub(crate) struct CreateBrushCommand {
    pub data: BrushData,
}

impl EditorCommand for CreateBrushCommand {
    fn execute(&mut self, world: &mut World) {
        spawn_brush_from_data(world, &self.data);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(entity) = entity_by_scene_node_id(world, self.data.node_id) {
            deselect_entities(world, &[entity]);
            despawn_scene_entity(world, entity);
        }
    }

    fn description(&self) -> &str {
        "Draw brush"
    }
}
