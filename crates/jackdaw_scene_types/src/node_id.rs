use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use rand::Rng;

use bevy::prelude::*;

/// Stable per-node identifier that persists in the scene document and is
/// attached to the spawned ECS entity. Lets a running game map a live entity
/// back to the authored scene node it came from (PIE "save runtime values"
/// needs this). The editor also restores selection across undo respawns
/// and tab swaps by matching on this.
///
/// Carried as a structural field on the scene node so it is the canonical
/// on-disk form rather than just another reflected component.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug, Reflect)]
#[reflect(Component, @crate::EditorHidden)]
pub struct SceneNodeId(pub u64);

/// Reflect type path for [`SceneNodeId`], used when projecting the structural
/// node id to and from the reflected-component representation.
pub const SCENE_NODE_ID_TYPE_PATH: &str = "jackdaw_scene_types::node_id::SceneNodeId";

/// Lower bound of the sparse id range. Ids below this are legacy values minted
/// by the old monotonic-from-1 counter and can collide across sessions.
pub const SPARSE_MIN: u64 = 1 << 32;
/// Upper bound of the random seed draw. The counter is seeded below this and
/// then advances freely, so minted ids are not clamped to it; the value leaves
/// headroom below `u64::MAX` for a session to mint without wrapping.
const SPARSE_MAX: u64 = 1 << 63;

/// Process-global source of fresh node ids, seeded once from a random base in
/// `[SPARSE_MIN, SPARSE_MAX)`. A random base keeps ids unique across processes
/// and across independently-authored files: two sessions land in different
/// ranges, so their ids never collide. Within a session ids stay monotonic.
static NEXT_NODE_ID: LazyLock<AtomicU64> =
    LazyLock::new(|| AtomicU64::new(rand::rng().random_range(SPARSE_MIN..SPARSE_MAX)));

impl SceneNodeId {
    /// Mint a fresh node id by advancing the global counter.
    pub fn next() -> Self {
        SceneNodeId(NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The type-path const must stay in lockstep with the reflected type
    /// path; both the PIE runtime and the load fallback use the const as a
    /// lookup key, so a silent drift here would break both.
    #[test]
    fn type_path_const_matches_reflected_type_path() {
        assert_eq!(SceneNodeId::type_path(), SCENE_NODE_ID_TYPE_PATH);
    }

    #[test]
    fn minted_ids_are_sparse_and_unique() {
        let a = SceneNodeId::next();
        let b = SceneNodeId::next();
        assert_ne!(a, b, "successive mints must differ");
        assert!(a.0 >= SPARSE_MIN, "minted id must be in the sparse range");
        assert!(b.0 >= SPARSE_MIN, "minted id must be in the sparse range");
    }
}
