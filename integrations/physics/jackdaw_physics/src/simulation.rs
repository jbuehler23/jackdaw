//! The physics tool's state, and keeping the simulation paused while the
//! tool is not running.

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// One run of the physics tool.
#[derive(Resource, Default)]
pub struct PhysicsToolState {
    /// Every body's transform when the tool started, put back on cancel and
    /// recorded as each moved body's starting point on commit.
    pub snapshots: HashMap<Entity, Transform>,
    /// Bodies the tool froze with `RigidBodyDisabled`, freed again when it
    /// ends.
    pub disabled_by_us: EntityHashSet,
    /// Whether the simulation has been started by a first drag.
    pub sim_active: bool,
    /// The grab under way, if any.
    pub drag: Option<PhysicsDrag>,
    /// Set by `physics.commit`; the settled bodies are recorded on the next
    /// frame.
    pub commit_requested: bool,
    /// Set once the settled bodies are recorded; the tool ends on its next
    /// frame.
    pub ended: bool,
}

/// Active drag: holds the primary entity being grabbed, the plane we project
/// the cursor onto, and the starting positions of ALL selected entities so
/// they move as a group.
#[derive(Clone)]
pub struct PhysicsDrag {
    pub entity: Entity,
    pub plane_origin: Vec3,
    pub plane_normal: Vec3,
    /// `entity_translation - cursor_hit` captured at drag start. Added to
    /// the new cursor-hit each frame so the pickup point on the entity
    /// stays under the cursor.
    pub grab_offset: Vec3,
    /// Starting position of the dragged entity at drag start.
    pub drag_start_pos: Vec3,
    /// Starting positions of ALL selected `RigidBody` entities at drag start.
    /// Used to move the entire group by the same delta.
    pub start_positions: HashMap<Entity, Vec3>,
    /// The camera and viewport node the drag started in, which it stays
    /// with wherever the cursor goes.
    pub camera: Entity,
    pub viewport: Entity,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PhysicsToolState>()
        .add_systems(Startup, pause_physics_on_startup);
}

/// The editor only simulates while the physics tool runs and a body has been
/// dragged.
fn pause_physics_on_startup(mut time: ResMut<Time<Physics>>) {
    time.pause();
}
