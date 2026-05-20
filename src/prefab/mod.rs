//! Flecs-style prefab system. A prefab is a scene-format file whose
//! root entity carries a `Prefab` component. Scenes reference prefabs
//! via an `IsA` relationship on an instance root entity; inherited
//! entities are materialized from the prefab AST, with sparse
//! per-field overrides layered on top.

pub mod cache;
pub mod canonical_path;
pub mod components;
pub mod operators;
pub mod overrides;
pub mod resolver;
pub mod save_load;
pub mod sync;
pub mod watcher;

pub use cache::PrefabAstCache;
pub use canonical_path::{CanonicalPrefabPath, canonical_prefab_path};
pub use components::{IsA, Prefab, PrefabEntityId};

use bevy::prelude::*;

pub struct PrefabPlugin;

impl Plugin for PrefabPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Prefab>()
            .register_type::<PrefabEntityId>()
            .register_type::<IsA>()
            .init_resource::<PrefabAstCache>()
            .init_resource::<sync::LastResolvedEpoch>()
            .add_systems(Update, sync::drive_respawn_on_prefab_cache_change);
    }
}
