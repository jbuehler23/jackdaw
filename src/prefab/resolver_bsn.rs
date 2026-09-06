//! Re-exports of the prefab resolver under the editor's names.
//!
//! The implementation lives in [`jackdaw_prefab`], shared with the game runtime
//! so a scene resolves the same way in the editor and in the game.

pub use jackdaw_prefab::components::{ISA_TYPE, PREFAB_ENTITY_ID_TYPE, PREFAB_TYPE};
pub use jackdaw_prefab::resolve::{
    CycleError, MAX_PREFAB_DEPTH, PrefabLookup, ResolveError, clone_scene, read_isa_deleted,
    read_isa_source, read_prefab_entity_id, resolve_scene, set_whole_component,
    sparsify_inherited_descendants, value_to_patch, would_cycle,
};
pub use jackdaw_prefab::source::{isa_value, relativize_isa_sources, source_path};
