use crate::commands::{EditorCommand, deselect_entities};
use crate::draw_brush::{BrushStableId, StableIdCounter, entity_by_stable_id};
use bevy::prelude::*;
use jackdaw_jsn::{Brush, BrushGroup};

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
    pub(crate) stable_id: BrushStableId,
    pub(crate) brush: Brush,
    pub(crate) transform: Transform,
    pub(crate) name: String,
    pub(crate) parent_stable_id: Option<BrushStableId>,
}

/// Either a single brush or a group containing child brushes.
#[derive(Clone)]
pub(crate) enum BrushOrGroup {
    Single(Box<BrushData>),
    Group {
        stable_id: BrushStableId,
        transform: Transform,
        name: String,
        parent_stable_id: Option<BrushStableId>,
        children: Vec<BrushData>,
    },
}

/// Read brush data from an existing entity. Lazily assigns a `BrushStableId` if missing.
pub(crate) fn brush_data_from_entity(world: &mut World, entity: Entity) -> BrushData {
    // Ensure the entity has a stable ID
    let stable_id = if let Some(sid) = world.get::<BrushStableId>(entity) {
        *sid
    } else {
        let sid = world.resource_mut::<StableIdCounter>().next();
        world.entity_mut(entity).insert(sid);
        sid
    };

    // Ensure parent has a stable ID too
    let parent_stable_id = if let Some(child_of) = world.get::<ChildOf>(entity) {
        let parent = child_of.0;
        if let Some(psid) = world.get::<BrushStableId>(parent) {
            Some(*psid)
        } else {
            let psid = world.resource_mut::<StableIdCounter>().next();
            world.entity_mut(parent).insert(psid);
            Some(psid)
        }
    } else {
        None
    };

    BrushData {
        stable_id,
        brush: world.get::<Brush>(entity).unwrap().clone(),
        transform: *world.get::<Transform>(entity).unwrap(),
        name: world
            .get::<Name>(entity)
            .map(std::string::ToString::to_string)
            .unwrap_or_default(),
        parent_stable_id,
    }
}

/// Spawn a brush entity from stored data. Returns new entity ID.
pub(crate) fn spawn_brush_from_data(world: &mut World, data: &BrushData) -> Entity {
    let parent_entity = data
        .parent_stable_id
        .and_then(|psid| entity_by_stable_id(world, psid));

    let mut ec = world.spawn((
        Name::new(data.name.clone()),
        data.brush.clone(),
        data.transform,
        data.stable_id,
        Visibility::default(),
    ));
    if let Some(parent) = parent_entity {
        ec.insert(ChildOf(parent));
    }
    let entity = ec.id();
    crate::scene_io::register_entity_in_ast(world, entity);
    entity
}

/// Spawn a brush or group from stored data. Returns top-level entity ID.
pub(crate) fn spawn_brush_or_group(world: &mut World, data: &BrushOrGroup) -> Entity {
    match data {
        BrushOrGroup::Single(brush_data) => spawn_brush_from_data(world, brush_data),
        BrushOrGroup::Group {
            stable_id,
            transform,
            name,
            parent_stable_id,
            children,
        } => {
            let parent_entity = parent_stable_id.and_then(|psid| entity_by_stable_id(world, psid));

            let mut ec = world.spawn((
                Name::new(name.clone()),
                BrushGroup,
                *transform,
                *stable_id,
                Visibility::default(),
            ));
            if let Some(p) = parent_entity {
                ec.insert(ChildOf(p));
            }
            let group_id = ec.id();
            crate::scene_io::register_entity_in_ast(world, group_id);
            for child in children {
                // Children reference the group by the group's stable_id which
                // we just spawned, so spawn_brush_from_data will find it.
                let mut child_data = child.clone();
                child_data.parent_stable_id = Some(*stable_id);
                spawn_brush_from_data(world, &child_data);
            }
            group_id
        }
    }
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
        if let Some(entity) = entity_by_stable_id(world, self.data.stable_id) {
            deselect_entities(world, &[entity]);
            world
                .resource_mut::<jackdaw_jsn::SceneJsnAst>()
                .remove_node(entity);
            if let Ok(entity_mut) = world.get_entity_mut(entity) {
                entity_mut.despawn();
            }
        }
    }

    fn description(&self) -> &str {
        "Draw brush"
    }
}
