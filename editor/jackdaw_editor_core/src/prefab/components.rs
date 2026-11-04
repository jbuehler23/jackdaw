//! Re-exports of the prefab vocabulary under the editor's names. Declared in
//! [`jackdaw_prefab`] so the game runtime reads the same components.

pub use jackdaw_prefab::components::{
    ISA_TYPE, IsA, PREFAB_ENTITY_ID_TYPE, PREFAB_TYPE, Prefab, PrefabEntityId,
};
