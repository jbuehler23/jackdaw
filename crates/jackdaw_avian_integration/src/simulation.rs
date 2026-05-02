//! Physics simulation support for the editor's Physics tool.
//!
//! This module adds `PhysicsPlugins` to the app and exposes `PhysicsToolState`
//!  -- a resource that the editor's Physics tool uses to track its active run
//! (snapshots for undo, dragged entity, disabled bodies, sim-activation
//! state). Physics is paused by default on startup; the tool unpauses it
//! when the user first drags a selected entity, and pauses it again on
//! tool exit.
//!
//! Wiring the tool lifecycle (entering/exiting the mode, committing
//! transforms, handling keys) lives in the main jackdaw crate  -- this crate
//! stays jackdaw-agnostic and only provides the avian hookup + state types.

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// Resource for the editor's Physics tool. Owned + mutated by tool systems
/// in the main crate; exposed here because we need the type in the
/// `PhysicsSimulationPlugin` registration.
#[derive(Resource, Default)]
pub struct PhysicsToolState {
    /// Transform snapshots captured on tool entry. Used on exit to diff
    /// against final transforms and build the undoable commit command.
    pub snapshots: HashMap<Entity, Transform>,
    /// Entities we inserted `RigidBodyDisabled` on during tool entry.
    /// Removed on exit so they resume normal simulation.
    pub disabled_by_us: EntityHashSet,
    /// Has the user initiated a drag yet this session? Gates whether
    /// `Time<Physics>` is unpaused.
    pub sim_active: bool,
    /// Currently-active drag grab (entity + drag-plane geometry).
    pub drag: Option<PhysicsDrag>,
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
}

pub struct PhysicsSimulationPlugin;

impl Plugin for PhysicsSimulationPlugin {
    fn build(&self, app: &mut App) {
        // `PhysicsPlugins` is owned by the hosting binary's
        // `main.rs`. Asserting presence here lets user
        // `MyGamePlugin`s add the same plugin without conflict.
        // This plugin only owns jackdaw-specific physics state.
        debug_assert!(
            app.is_plugin_added::<PhysicsSchedulePlugin>(),
            "PhysicsSimulationPlugin requires PhysicsPlugins first; \
             add `PhysicsPlugins::default()` in main.rs before EditorPlugins."
        );
        app.init_resource::<PhysicsToolState>()
            .add_systems(Startup, pause_physics_on_startup);
    }
}

/// Physics stays paused by default; the editor only runs it
/// when the Physics tool is active and the user is dragging.
fn pause_physics_on_startup(mut time: ResMut<Time<Physics>>) {
    time.pause();
}
