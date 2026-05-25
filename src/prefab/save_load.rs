//! Glue between the editor's `scene_io` and the prefab cache / resolver.

use crate::prefab::cache::PrefabAstCache;
use jackdaw_jsn::SceneJsnAst;
use jackdaw_jsn::format::JsnEntity;
use std::path::{Path, PathBuf};

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";

/// Walk a freshly-loaded scene AST for `IsA` references and load /
/// cache each referenced prefab. Returns the list of prefab paths
/// the watcher should track.
pub fn populate_cache_for_scene(
    ast: &SceneJsnAst,
    cache: &mut PrefabAstCache,
    scene_dir: &Path,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for key in ast.entities_with_component(ISA_TYPE) {
        let Some(isa) = ast.get_component_at(key, ISA_TYPE) else {
            continue;
        };
        let Some(source) = isa.get("source").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = resolve_source_path(source, scene_dir);
        if cache.get(&path).is_none()
            && let Ok(prefab_ast) = read_prefab_ast(&path)
        {
            cache.insert(path.clone(), prefab_ast);
        }
        paths.push(path);
    }
    paths
}

fn resolve_source_path(source: &str, scene_dir: &Path) -> PathBuf {
    let p = Path::new(source);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        scene_dir.join(p)
    }
}

fn read_prefab_ast(path: &Path) -> Result<SceneJsnAst, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    let scene: jackdaw_jsn::format::JsnScene = serde_json::from_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(SceneJsnAst::from_jsn_scene(&scene, &[]))
}

/// Replace each component value of an instance-rooted entity with the
/// sparse delta against its corresponding prefab entity. Entities with
/// no `IsA` ancestor pass through unchanged.
pub fn sparsify_instance_entities(
    entities: Vec<JsnEntity>,
    cache: &PrefabAstCache,
    scene_dir: &Path,
) -> Vec<JsnEntity> {
    entities
        .into_iter()
        .map(|entity| sparsify_one(entity, cache, scene_dir))
        .collect()
}

fn sparsify_one(mut entity: JsnEntity, cache: &PrefabAstCache, scene_dir: &Path) -> JsnEntity {
    let Some(isa) = entity.components.get(ISA_TYPE).cloned() else {
        return entity;
    };
    let Some(source) = isa.get("source").and_then(|v| v.as_str()) else {
        return entity;
    };
    let path = resolve_source_path(source, scene_dir);
    let Some(prefab) = cache.get(&path) else {
        return entity;
    };
    // Match by PrefabEntityId. Default to 0 (the prefab root) if absent.
    let id = entity
        .components
        .get(PREFAB_ENTITY_ID_TYPE)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    let prefab_match = prefab.nodes.iter().find(|n| {
        n.components
            .get(PREFAB_ENTITY_ID_TYPE)
            .and_then(serde_json::Value::as_u64)
            == Some(id as u64)
    });
    let Some(prefab_match) = prefab_match else {
        return entity;
    };

    let component_keys: Vec<String> = entity.components.keys().cloned().collect();
    for type_path in component_keys {
        // Never sparsify markers themselves; they're load-bearing for
        // the resolver to find and rebuild the instance subtree.
        if type_path == ISA_TYPE || type_path == PREFAB_ENTITY_ID_TYPE || type_path == PREFAB_TYPE {
            continue;
        }
        let Some(current) = entity.components.get(&type_path).cloned() else {
            continue;
        };
        let Some(inherited) = prefab_match.components.get(&type_path) else {
            // Component exists on instance but not on prefab; keep verbatim.
            continue;
        };
        match shallow_diff(inherited, &current) {
            None => {
                // Identical to prefab; drop entirely.
                entity.components.remove(&type_path);
            }
            Some(delta) => {
                entity.components.insert(type_path, delta);
            }
        }
    }
    entity
}

/// Minimal sparse JSON value such that
/// `apply_deltas(prefab_value.clone(), &diff) == current`.
/// Returns `None` when `prefab_value == current`.
pub fn shallow_diff(
    prefab_value: &serde_json::Value,
    current: &serde_json::Value,
) -> Option<serde_json::Value> {
    if prefab_value == current {
        return None;
    }
    if let (serde_json::Value::Object(p), serde_json::Value::Object(c)) = (prefab_value, current) {
        let mut delta = serde_json::Map::new();
        for (key, current_v) in c {
            match p.get(key) {
                Some(prefab_v) if prefab_v == current_v => {}
                Some(prefab_v) => {
                    if let Some(sub) = shallow_diff(prefab_v, current_v) {
                        delta.insert(key.clone(), sub);
                    }
                    // If the parent comparison flagged them as different but
                    // the recursive diff finds no field-level changes, that
                    // means the values are field-order-sensitive but
                    // content-equivalent. Treat as no-change.
                }
                None => {
                    delta.insert(key.clone(), current_v.clone());
                }
            }
        }
        if delta.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(delta))
        }
    } else {
        Some(current.clone())
    }
}
