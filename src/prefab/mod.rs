//! Flecs-style prefab system. A prefab is a scene-format file whose
//! root entity carries a `Prefab` component. Scenes reference prefabs
//! via an `IsA` relationship on an instance root entity; inherited
//! entities are materialized from the prefab AST, with sparse
//! per-field overrides layered on top.

pub mod cache;
pub mod canonical_path;
pub mod components;
pub mod operators;
pub mod overrides_bsn;
pub mod resolver_bsn;
pub mod save_load;
pub mod sync;
pub mod watcher;

pub use cache::PrefabAstCache;
pub use canonical_path::{CanonicalPrefabPath, canonical_prefab_path};
pub use components::{IsA, Prefab, PrefabEntityId};

use bevy::prelude::*;
use jackdaw_scene_types::UiSceneRoot;

/// Query filter for a UI scene root the open document owns: the scene this tab
/// is editing, rather than one it instances.
///
/// A UI scene imported into a world scene has the same shape as the one being
/// edited: `UiSceneRoot` and no `ChildOf`, since a UI root must stay an ECS
/// root for Bevy to lay it out. A filter of `(With<UiSceneRoot>,
/// Without<ChildOf>)` alone therefore also matches a world scene's imported
/// overlay.
///
/// `IsA` alone cannot discriminate either, because a root can carry it and
/// still be the document's own: `save_as_variant` stamps a variant root
/// `Prefab + PrefabEntityId(0) + IsA(original)` and copies the resolved
/// components across, so a variant saved off an imported UI overlay is a UI
/// scene file whose root has both. The discriminator is `Prefab`, the marker
/// for a file's own root: `save_as_variant` and `normalize_as_prefab_source`
/// stamp it, and an instance cannot acquire one, since
/// `inherit_root_components` filters `Prefab` and `IsA` out of what an instance
/// inherits.
pub type AuthoredUiSceneRoot = (
    With<UiSceneRoot>,
    Without<ChildOf>,
    Or<(Without<IsA>, With<Prefab>)>,
);

/// Query filter for a UI scene root imported into this document as a prefab
/// instance: the complement of [`AuthoredUiSceneRoot`], which documents both.
pub type ImportedUiSceneRoot = (
    With<UiSceneRoot>,
    Without<ChildOf>,
    With<IsA>,
    Without<Prefab>,
);

pub struct PrefabPlugin;

impl Plugin for PrefabPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(jackdaw_prefab::PrefabTypesPlugin)
            .init_resource::<PrefabAstCache>()
            .init_resource::<sync::LastResolvedEpoch>()
            .add_systems(
                Update,
                (
                    sync::drive_respawn_on_prefab_cache_change,
                    operators::poll_prefab_pick.run_if(in_state(crate::AppState::Editor)),
                ),
            );
    }
}
