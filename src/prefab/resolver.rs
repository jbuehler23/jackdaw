//! Overlay a cached prefab AST under an instance node to produce a
//! resolved scene AST. Each `IsA` node in the input AST gets its
//! inherited subtree materialized, with the instance's sparse field
//! deltas applied on top.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::prefab::cache::PrefabAstCache;
use crate::prefab::overrides;
use jackdaw_jsn::SceneJsnAst;

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

#[derive(Debug)]
pub struct CycleError {
    pub chain: Vec<PathBuf>,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "prefab IsA cycle:")?;
        for path in &self.chain {
            write!(f, " {}", path.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for CycleError {}

/// Returns `Some(err)` if visiting `next` while currently resolving
/// the entries in `chain` would form a cycle. The detector treats
/// `chain` as an inclusive list of paths currently being expanded.
pub fn would_cycle(chain: &[PathBuf], next: &Path) -> Option<CycleError> {
    if chain.iter().any(|p| p.as_path() == next) {
        let mut cycle = chain.to_vec();
        cycle.push(next.to_path_buf());
        Some(CycleError { chain: cycle })
    } else {
        None
    }
}

#[derive(Debug)]
pub enum ResolveError {
    Cycle(CycleError),
    PrefabNotCached(PathBuf),
    BadIsA(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle(c) => write!(f, "{c}"),
            Self::PrefabNotCached(p) => write!(f, "prefab not cached: {}", p.display()),
            Self::BadIsA(s) => write!(f, "bad IsA: {s}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve all `IsA` instances in `scene_ast` against `cache`. Returns
/// a new AST where every instance has its inherited subtree materialized,
/// with sparse overrides applied.
pub fn resolve_scene(
    scene_ast: &SceneJsnAst,
    cache: &PrefabAstCache,
) -> Result<SceneJsnAst, ResolveError> {
    let mut resolved = scene_ast.clone();
    let chain: Vec<PathBuf> = Vec::new();
    expand_instances(&mut resolved, cache, &chain)?;
    Ok(resolved)
}

fn expand_instances(
    ast: &mut SceneJsnAst,
    cache: &PrefabAstCache,
    chain: &[PathBuf],
) -> Result<(), ResolveError> {
    let isa_keys: Vec<usize> = ast.entities_with_component(ISA_TYPE).collect();
    for key in isa_keys {
        let isa_value = ast
            .get_component_at(key, ISA_TYPE)
            .cloned()
            .ok_or_else(|| ResolveError::BadIsA(format!("missing IsA on key {key}")))?;
        let source = isa_value
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ResolveError::BadIsA("IsA.source missing".into()))?;
        let source_path = PathBuf::from(source);
        if let Some(err) = would_cycle(chain, &source_path) {
            return Err(ResolveError::Cycle(err));
        }
        let prefab = cache
            .get(&source_path)
            .ok_or_else(|| ResolveError::PrefabNotCached(source_path.clone()))?;
        let deleted: Vec<u32> = isa_value
            .get("deleted")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|u| u as u32))
                    .collect()
            })
            .unwrap_or_default();

        let mut next_chain = chain.to_vec();
        next_chain.push(source_path.clone());

        let mut prefab_clone = prefab.clone();
        expand_instances(&mut prefab_clone, cache, &next_chain)?;

        merge_prefab_under_instance(ast, key, &prefab_clone, &deleted);
    }
    Ok(())
}

fn merge_prefab_under_instance(
    ast: &mut SceneJsnAst,
    instance_root: usize,
    prefab: &SceneJsnAst,
    deleted: &[u32],
) {
    let Some(prefab_root) = prefab
        .roots()
        .find(|k| prefab.get_component_at(*k, PREFAB_TYPE).is_some())
    else {
        return;
    };

    let existing_ids: std::collections::HashSet<u32> = ast
        .descendants_of(instance_root)
        .into_iter()
        .filter_map(|k| {
            ast.get_component_at(k, PREFAB_ENTITY_ID_TYPE)
                .and_then(serde_json::Value::as_u64)
                .map(|u| u as u32)
        })
        .collect();

    // Materialize the prefab root's components onto the instance root.
    // `Name`, `Transform`, etc. on the prefab root become the instance's
    // default values; this is what makes IsA an inheritance relation
    // rather than just a child pointer.
    //
    // When the instance already authors a component, the stored value
    // is treated as a sparse delta and merged onto the inherited base.
    // Same semantics as the descendant override loop below; lets callers
    // write `Transform = { translation: [x,y,z] }` without having to
    // restate every other field.
    if let Some(prefab_root_components) = prefab.components_at(prefab_root) {
        let inherited: Vec<(String, serde_json::Value)> = prefab_root_components
            .iter()
            .filter(|(k, _)| k.as_str() != PREFAB_TYPE && k.as_str() != ISA_TYPE)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (type_path, base) in inherited {
            let instance_delta = ast.get_component_at(instance_root, &type_path).cloned();
            match instance_delta {
                Some(delta) => {
                    let mut merged = base;
                    if overrides::apply_deltas(&mut merged, &delta).is_ok() {
                        ast.replace_component(instance_root, &type_path, merged);
                    }
                }
                None => {
                    ast.insert_component(instance_root, &type_path, base);
                }
            }
        }
    }

    let prefab_descendants = prefab.descendants_of(prefab_root);
    let mut prefab_idx_to_scene_idx: std::collections::HashMap<usize, usize> =
        std::collections::HashMap::new();
    prefab_idx_to_scene_idx.insert(prefab_root, instance_root);

    for prefab_idx in &prefab_descendants {
        let id = prefab
            .get_component_at(*prefab_idx, PREFAB_ENTITY_ID_TYPE)
            .and_then(serde_json::Value::as_u64)
            .map(|u| u as u32);
        let Some(id) = id else { continue };
        if deleted.contains(&id) {
            continue;
        }
        if existing_ids.contains(&id) {
            continue;
        }
        let prefab_parent = prefab.nodes[*prefab_idx].parent.unwrap_or(prefab_root);
        let scene_parent = *prefab_idx_to_scene_idx
            .get(&prefab_parent)
            .unwrap_or(&instance_root);
        let mut single = SceneJsnAst::default();
        single.nodes.push(prefab.nodes[*prefab_idx].clone());
        single.nodes[0].parent = None;
        let new_idx = ast.clone_node_into(&single, 0, scene_parent);
        prefab_idx_to_scene_idx.insert(*prefab_idx, new_idx);
    }

    let override_keys: Vec<usize> = ast
        .descendants_of(instance_root)
        .into_iter()
        .filter(|k| ast.get_component_at(*k, PREFAB_ENTITY_ID_TYPE).is_some())
        .collect();
    for ok in override_keys {
        let id = ast
            .get_component_at(ok, PREFAB_ENTITY_ID_TYPE)
            .and_then(serde_json::Value::as_u64)
            .map(|u| u as u32);
        let Some(id) = id else { continue };
        let prefab_match = prefab.descendants_of(prefab_root).into_iter().find(|k| {
            prefab
                .get_component_at(*k, PREFAB_ENTITY_ID_TYPE)
                .and_then(serde_json::Value::as_u64)
                == Some(id as u64)
        });
        let Some(prefab_match) = prefab_match else {
            continue;
        };

        let component_pairs: Vec<(String, serde_json::Value)> = ast
            .components_at(ok)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        for (type_path, delta_value) in component_pairs {
            if type_path == PREFAB_ENTITY_ID_TYPE || type_path == ISA_TYPE {
                continue;
            }
            if let Some(inherited) = prefab.get_component_at(prefab_match, &type_path).cloned() {
                let mut merged = inherited;
                if overrides::apply_deltas(&mut merged, &delta_value).is_ok() {
                    ast.replace_component(ok, &type_path, merged);
                }
            }
        }
    }
}
