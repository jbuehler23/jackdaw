//! Backend-neutral scene snapshots for undo.
//!
//! The operator dispatcher captures a snapshot before invoke and diffs
//! after. The snapshot format is behind a trait so the current
//! `SceneJsnAst`-backed implementation can be swapped for a BSN-backed
//! one without touching the dispatcher or extension authors' code.

use std::any::Any;

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;

/// A point-in-time representation of the editor scene.
pub trait SceneSnapshot: Any + Send + Sync + 'static {
    /// Make the world match this snapshot. Used for both undo and redo.
    fn apply(&self, world: &mut World);

    /// Whether this snapshot represents the same scene as `other`. Used
    /// by the dispatcher to skip pushing history entries when an
    /// operator finished without actually changing the scene.
    fn equals(&self, other: &dyn SceneSnapshot) -> bool;

    fn clone_box(&self) -> Box<dyn SceneSnapshot>;

    /// Implementors should forward to `self`. Needed for downcasting
    /// inside [`Self::equals`].
    fn as_any(&self) -> &dyn Any;
}

/// Strategy for producing snapshots from the current world.
///
/// Takes `&mut World` because concrete snapshotters typically walk
/// every entity via `world.query_filtered(...)` / `world.entity_mut(...)`,
/// which Bevy requires exclusive access for (query-state caching).
pub trait SceneSnapshotter: Send + Sync + 'static {
    fn capture(&self, world: &mut World) -> Box<dyn SceneSnapshot>;
}

/// The active snapshotter. Inserted once at plugin setup. Swapped on
/// BSN migration.
#[derive(Resource, Deref, DerefMut)]
pub struct ActiveSnapshotter(pub Box<dyn SceneSnapshotter>);
