#![deny(missing_docs)]
//! Prefab instancing for jackdaw scenes.
//!
//! A prefab is an ordinary `.bsn` document whose root carries [`Prefab`].
//! A scene instances one by giving an entity an [`IsA`] pointing at that
//! file; every entity inside the prefab carries a [`PrefabEntityId`], and
//! the scene stores only the fields that differ from what it inherits.
//!
//! [`resolve_scene`] turns that sparse form into a complete document, with
//! inherited subtrees materialized and overrides merged on top;
//! [`sparsify_inherited_descendants`] takes it back down again for saving.
//! The editor and the game runtime both resolve through this crate, so a
//! scene renders the same in each.

pub mod components;
pub mod resolve;
pub mod source;

pub use components::{ISA_TYPE, IsA, PREFAB_ENTITY_ID_TYPE, PREFAB_TYPE, Prefab, PrefabEntityId};
pub use resolve::{
    CycleError, MAX_PREFAB_DEPTH, PrefabLookup, ResolveError, clone_scene, read_isa_deleted,
    read_isa_source, read_prefab_entity_id, resolve_scene, set_whole_component,
    sparsify_inherited_descendants, value_to_patch, would_cycle,
};
pub use source::{
    absolutize_isa_sources, isa_value, normalize_as_prefab_source, normalize_path,
    read_prefab_document, relativize_isa_sources, source_path,
};

use bevy::prelude::*;

/// Registers the prefab vocabulary for reflection.
///
/// A document naming a component the registry does not hold loads without
/// it, so an app that reads scenes carrying prefab instances needs this
/// plugin before the first load.
pub struct PrefabTypesPlugin;

impl Plugin for PrefabTypesPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Prefab>()
            .register_type::<PrefabEntityId>()
            .register_type::<IsA>();
    }
}
