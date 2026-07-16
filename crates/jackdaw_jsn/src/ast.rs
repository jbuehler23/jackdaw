use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::format::{JsnAssets, JsnEntity, JsnMetadata, JsnScene};

/// Lower bound of the sparse id range.
use jackdaw_scene_types::SPARSE_MIN;
/// Stable per-node identity, format-independent (identity, not JSON), so it
/// lives in `jackdaw_scene_types`. `JsnEntityNode` (`jackdaw_jsn`'s own API)
/// stores it directly rather than re-exporting the type under a local alias.
use jackdaw_scene_types::SceneNodeId;

/// Report whether a loaded scene carries node ids that break the global-key
/// invariant: any duplicate id, any id below `SPARSE_MIN` (minted by the old
/// counter), or any missing id. Such scenes are re-minted on load.
pub fn needs_id_migration(scene: &JsnScene) -> bool {
    let mut seen = HashSet::new();
    for entity in &scene.scene {
        match entity.id {
            Some(id) if id >= SPARSE_MIN => {
                if !seen.insert(id) {
                    return true;
                }
            }
            // Below the sparse range, or no id at all: legacy, must heal.
            _ => return true,
        }
    }
    false
}

/// In-memory form of a legacy `.jsn` scene, used by the load-time id heal and
/// the `.jsn` -> BSN conversion tooling. The live editor document is the BSN
/// AST; this type only bridges legacy files at the disk boundary.
#[derive(Resource, Default, Clone, PartialEq)]
pub struct SceneJsnAst {
    /// Entity nodes, indexed by position.
    pub nodes: Vec<JsnEntityNode>,
    /// Map from ECS preview entity to node index.
    pub ecs_to_jsn: HashMap<Entity, usize>,
    /// Indices of nodes whose ECS preview entities need re-sync.
    pub dirty_indices: HashSet<usize>,
    /// Inline assets table (materials, images, etc.).
    pub assets: JsnAssets,
}

/// A single entity in the scene document.
///
/// Mirrors `JsnEntity` from the file format  -- `name` and `parent` are
/// structural fields, everything else (Transform, Visibility, Brush, etc.)
/// lives in `components` as `serde_json::Value`.
#[derive(Clone, PartialEq)]
pub struct JsnEntityNode {
    /// Stable id for this node, persisted in the `.jsn` and attached to the
    /// spawned ECS entity. `None` only transiently before a fresh id is
    /// minted by `from_jsn_scene`.
    pub id: Option<SceneNodeId>,
    /// Parent index into `SceneJsnAst::nodes`.
    pub parent: Option<usize>,
    /// All component data keyed by type path (e.g. `"bevy_transform::components::transform::Transform"`).
    /// Includes Name, Transform, Visibility  -- everything is a component.
    pub components: HashMap<String, serde_json::Value>,
    /// Components auto-added via Bevy's `#[require]` attributes (e.g., avian's
    /// `Position`, `ColliderAabb`, `ComputedMass`, etc.). Never serialized to
    /// the scene file; they are recreated at runtime.
    pub derived_components: HashSet<String>,
    /// The ECS entity used to preview this node in the viewport.
    pub ecs_entity: Option<Entity>,
}

impl SceneJsnAst {
    /// Populate from a loaded `JsnScene` and the ECS entities that were spawned for it.
    ///
    /// `entity_map` maps JSN entity index -> spawned ECS entity.
    pub fn from_jsn_scene(scene: &JsnScene, entity_map: &[Entity]) -> Self {
        let mut ecs_to_jsn = HashMap::new();
        let mut nodes: Vec<JsnEntityNode> = scene
            .scene
            .iter()
            .enumerate()
            .map(|(i, jsn)| {
                let ecs_entity = entity_map.get(i).copied();
                if let Some(e) = ecs_entity {
                    ecs_to_jsn.insert(e, i);
                }
                // The structural `id` is canonical. Fall back to a stray
                // `SceneNodeId` reflected into `components` (defensive against
                // older save paths), and finally mint a fresh id so every
                // loaded node is identifiable.
                let mut components = jsn.components.clone();
                let id = jsn
                    .id
                    .map(SceneNodeId)
                    .or_else(|| {
                        components
                            .remove(jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH)
                            .as_ref()
                            .and_then(serde_json::Value::as_u64)
                            .map(SceneNodeId)
                    })
                    .unwrap_or_else(SceneNodeId::next);
                JsnEntityNode {
                    id: Some(id),
                    parent: jsn.parent,
                    components,
                    derived_components: HashSet::new(),
                    ecs_entity,
                }
            })
            .collect();

        // Older scenes minted ids from a per-process counter that reset every
        // run, so a loaded scene can carry duplicate or low-range ids that
        // collapse distinct nodes onto one entity in the by-id match. Re-mint
        // every node to a sparse id when that is detected. Parent links are
        // stored by index, so they are unaffected.
        if needs_id_migration(scene) {
            for node in &mut nodes {
                node.id = Some(SceneNodeId::next());
            }
        }

        Self {
            nodes,
            ecs_to_jsn,
            dirty_indices: HashSet::new(),
            assets: scene.assets.clone(),
        }
    }

    /// Emit a `JsnScene` for serialization to disk.
    pub fn to_jsn_scene(&self, metadata: JsnMetadata) -> JsnScene {
        let scene = self
            .nodes
            .iter()
            .map(|node| JsnEntity {
                id: node.id.map(|id| id.0),
                parent: node.parent,
                components: node.components.clone(),
            })
            .collect();

        JsnScene {
            jsn: crate::format::JsnHeader::default(),
            metadata,
            assets: self.assets.clone(),
            editor: None,
            scene,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `JsnScene` from `(id, parent)` pairs. `id == None` omits the
    /// structural id; `parent == None` omits the parent pointer.
    fn scene_from_nodes(nodes: &[(Option<u64>, Option<usize>)]) -> JsnScene {
        let scene: Vec<_> = nodes
            .iter()
            .map(|(id, parent)| {
                let mut e = serde_json::json!({ "components": {} });
                if let Some(id) = id {
                    e["id"] = serde_json::json!(id);
                }
                if let Some(parent) = parent {
                    e["parent"] = serde_json::json!(parent);
                }
                e
            })
            .collect();
        let json = serde_json::json!({
            "jsn": { "format_version": [3, 0, 0], "editor_version": "test", "bevy_version": "0.18" },
            "metadata": { "name": "t" },
            "assets": {},
            "editor": null,
            "scene": scene,
        });
        serde_json::from_value(json).expect("scene should parse")
    }

    /// Healthy sparse ids survive a `from_jsn_scene` / `to_jsn_scene`
    /// round trip unchanged.
    #[test]
    fn node_ids_survive_load_save_round_trip() {
        let scene = scene_from_nodes(&[
            (Some(SPARSE_MIN + 5), None),
            (Some(SPARSE_MIN + 6), Some(0)),
        ]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        let emitted = ast.to_jsn_scene(JsnMetadata::default());
        assert_eq!(emitted.scene[0].id, Some(SPARSE_MIN + 5));
        assert_eq!(emitted.scene[1].id, Some(SPARSE_MIN + 6));
        assert_eq!(emitted.scene[1].parent, Some(0));
    }

    #[test]
    fn needs_migration_detects_duplicate_ids() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN), None), (Some(SPARSE_MIN), None)]);
        assert!(
            needs_id_migration(&scene),
            "two equal ids must trigger migration"
        );
    }

    #[test]
    fn needs_migration_detects_legacy_low_ids() {
        let scene = scene_from_nodes(&[(Some(9), None), (Some(10), None)]);
        assert!(
            needs_id_migration(&scene),
            "ids below SPARSE_MIN must trigger migration"
        );
    }

    #[test]
    fn needs_migration_detects_missing_id() {
        let scene = scene_from_nodes(&[(None, None)]);
        assert!(
            needs_id_migration(&scene),
            "a node with no id must trigger migration"
        );
    }

    #[test]
    fn needs_migration_false_for_sparse_unique() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN), None), (Some(SPARSE_MIN + 1), None)]);
        assert!(
            !needs_id_migration(&scene),
            "distinct sparse ids need no migration"
        );
    }

    /// Minted ids land in the sparse range and never repeat back-to-back.
    #[test]
    fn minted_ids_are_sparse_and_unique() {
        let a = SceneNodeId::next();
        let b = SceneNodeId::next();
        assert_ne!(a, b, "successive mints must differ");
        assert!(a.0 >= SPARSE_MIN, "minted id must be in the sparse range");
        assert!(b.0 >= SPARSE_MIN, "minted id must be in the sparse range");
    }

    /// Colliding ids are healed to unique sparse ids on load.
    #[test]
    fn from_jsn_scene_dedupes_colliding_ids() {
        let scene = scene_from_nodes(&[(Some(10), None), (Some(10), None), (Some(10), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        let ids: Vec<u64> = ast
            .nodes
            .iter()
            .map(|n| n.id.expect("healed id").0)
            .collect();
        let unique: HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "healed ids must be unique");
        assert!(
            ids.iter().all(|id| *id >= SPARSE_MIN),
            "healed ids must be sparse"
        );
    }

    /// Unique but legacy-low ids are lifted into the sparse range on load.
    #[test]
    fn from_jsn_scene_remints_legacy_low_ids() {
        let scene = scene_from_nodes(&[(Some(1), None), (Some(2), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert!(
            ast.nodes
                .iter()
                .all(|n| n.id.expect("healed id").0 >= SPARSE_MIN)
        );
    }

    /// A scene whose ids are already sparse and unique loads untouched.
    #[test]
    fn from_jsn_scene_preserves_sparse_unique_ids() {
        let scene = scene_from_nodes(&[(Some(SPARSE_MIN + 5), None), (Some(SPARSE_MIN + 6), None)]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert_eq!(ast.nodes[0].id, Some(SceneNodeId(SPARSE_MIN + 5)));
        assert_eq!(ast.nodes[1].id, Some(SceneNodeId(SPARSE_MIN + 6)));
    }

    /// Re-minting changes ids but leaves the index-based parent links intact.
    #[test]
    fn from_jsn_scene_remint_preserves_parent_links() {
        let scene = scene_from_nodes(&[(Some(10), None), (Some(10), Some(0))]);
        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        assert_eq!(
            ast.nodes[1].parent,
            Some(0),
            "parent index survives re-mint"
        );
        assert_ne!(
            ast.nodes[0].id, ast.nodes[1].id,
            "ids are unique after heal"
        );
    }

    /// A legacy `JsnScene` with no `id` field still loads, minting fresh ids
    /// for every node that lacks one.
    #[test]
    fn legacy_scene_without_id_mints_fresh_ids() {
        let json = serde_json::json!({
            "jsn": {
                "format_version": [3, 0, 0],
                "editor_version": "test",
                "bevy_version": "0.18"
            },
            "metadata": { "name": "legacy" },
            "assets": {},
            "editor": null,
            "scene": [
                { "components": {} },
                { "parent": 0, "components": {} }
            ]
        });
        let scene: JsnScene = serde_json::from_value(json).expect("legacy scene should parse");
        assert_eq!(scene.scene[0].id, None, "legacy entity has no on-disk id");

        let ast = SceneJsnAst::from_jsn_scene(&scene, &[]);
        let id0 = ast.nodes[0]
            .id
            .expect("legacy node 0 should be minted an id");
        let id1 = ast.nodes[1]
            .id
            .expect("legacy node 1 should be minted an id");
        assert_ne!(id0, id1, "minted ids must be distinct");
    }
}
