//! Path-walking applier that overlays sparse field deltas onto a base
//! JSON value. The base is the inherited component value from the
//! prefab AST; the deltas are the instance's sparse override entries.

use jackdaw_jsn::SceneJsnAst;
use serde_json::Value;
use std::path::PathBuf;

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";

#[derive(Debug)]
pub enum ApplyError {
    PathBeyondLeaf(String),
    UnexpectedShape(String),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathBeyondLeaf(p) => write!(f, "override path beyond leaf: {p}"),
            Self::UnexpectedShape(s) => write!(f, "unexpected shape: {s}"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// Apply `deltas` onto `base` in-place. Each key in `deltas` is
/// either a JSON field name or a dot-path into nested objects. Values
/// can be scalars (set the leaf) or nested objects (recurse).
pub fn apply_deltas(base: &mut Value, deltas: &Value) -> Result<(), ApplyError> {
    let Value::Object(delta_map) = deltas else {
        return Err(ApplyError::UnexpectedShape(
            "deltas must be a JSON object".into(),
        ));
    };
    for (key, value) in delta_map {
        if key.contains('.') {
            set_at_path(base, key, value.clone())?;
        } else if value.is_object() {
            let entry = base.as_object_mut().and_then(|m| m.get_mut(key.as_str()));
            match entry {
                Some(existing) if existing.is_object() => {
                    apply_deltas(existing, value)?;
                }
                Some(other) => *other = value.clone(),
                None => {
                    if let Value::Object(base_map) = base {
                        base_map.insert(key.clone(), value.clone());
                    } else {
                        return Err(ApplyError::UnexpectedShape(
                            "base must be a JSON object for object delta".into(),
                        ));
                    }
                }
            }
        } else if let Value::Object(base_map) = base {
            base_map.insert(key.clone(), value.clone());
        } else {
            return Err(ApplyError::UnexpectedShape(
                "base must be a JSON object for scalar delta".into(),
            ));
        }
    }
    Ok(())
}

fn set_at_path(base: &mut Value, dot_path: &str, value: Value) -> Result<(), ApplyError> {
    let mut cursor = base;
    let parts: Vec<&str> = dot_path.split('.').collect();
    let (last, head) = parts.split_last().unwrap();
    for part in head {
        let next = cursor
            .as_object_mut()
            .ok_or_else(|| ApplyError::PathBeyondLeaf(dot_path.into()))?
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Default::default()));
        cursor = next;
    }
    let leaf_map = cursor
        .as_object_mut()
        .ok_or_else(|| ApplyError::PathBeyondLeaf(dot_path.into()))?;
    leaf_map.insert((*last).to_string(), value);
    Ok(())
}

/// True if `entity_key`'s component value at `type_path` (or one of its
/// dot-path subfields, if `field_path` is `Some`) differs from the
/// corresponding value in the cached prefab AST that the entity inherits
/// from. False when the entity isn't inside an `IsA` instance, when no
/// prefab match exists, or when the values are equal.
///
/// Returns false on any structural mismatch (missing key, etc.) so the
/// caller can treat that as "no override" without panicking.
pub fn field_is_overridden(
    scene_ast: &SceneJsnAst,
    cache: &crate::prefab::cache::PrefabAstCache,
    entity_key: usize,
    type_path: &str,
    field_path: Option<&str>,
) -> bool {
    let Some(scene_value) = scene_ast.get_component_at(entity_key, type_path) else {
        return false;
    };

    let Some((prefab_path, prefab_entity_id)) = resolve_inheritance(scene_ast, entity_key) else {
        return false;
    };

    let Some(prefab_ast) = cache.get(&prefab_path) else {
        return false;
    };

    let prefab_key = prefab_ast.nodes.iter().enumerate().find_map(|(i, node)| {
        let id = node
            .components
            .get(PREFAB_ENTITY_ID_TYPE)
            .and_then(serde_json::Value::as_u64)?;
        if id as u32 == prefab_entity_id {
            Some(i)
        } else {
            None
        }
    });
    let Some(prefab_key) = prefab_key else {
        return false;
    };

    let Some(prefab_value) = prefab_ast.get_component_at(prefab_key, type_path) else {
        // Component exists on the instance but not on the prefab. That's an addition.
        return true;
    };

    match field_path {
        None => scene_value != prefab_value,
        Some(path) => {
            let scene_leaf = walk_dot_path(scene_value, path);
            let prefab_leaf = walk_dot_path(prefab_value, path);
            scene_leaf != prefab_leaf
        }
    }
}

/// True if `entity_key`'s AST node sits inside a prefab-instance
/// subtree. The node must carry a `PrefabEntityId` (so it's tracked as
/// a prefab descendant) AND have an `IsA` ancestor (so it actually
/// inherits from a prefab file). The check is inclusive of the entity
/// itself; an instance root carries both components on the same node.
pub fn is_inside_prefab_instance(ast: &SceneJsnAst, entity_key: usize) -> bool {
    ast.get_component_at(entity_key, PREFAB_ENTITY_ID_TYPE)
        .is_some()
        && ast.ancestor_with_component(entity_key, ISA_TYPE).is_some()
}

/// Walk up `entity_key`'s parent chain until we find a node carrying an
/// `IsA` component. Returns the prefab source path and the
/// `PrefabEntityId` on the original `entity_key` (which identifies which
/// prefab descendant the entity corresponds to).
pub(crate) fn resolve_inheritance(ast: &SceneJsnAst, entity_key: usize) -> Option<(PathBuf, u32)> {
    let prefab_entity_id = ast
        .get_component_at(entity_key, PREFAB_ENTITY_ID_TYPE)
        .and_then(serde_json::Value::as_u64)
        .map(|u| u as u32)?;

    let mut current = entity_key;
    loop {
        if let Some(isa) = ast.get_component_at(current, ISA_TYPE) {
            let source = isa.get("source").and_then(|v| v.as_str())?;
            return Some((PathBuf::from(source), prefab_entity_id));
        }
        let parent = ast.nodes.get(current)?.parent?;
        current = parent;
    }
}

fn walk_dot_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cursor = value;
    for part in path.split('.') {
        cursor = cursor.as_object()?.get(part)?;
    }
    Some(cursor)
}
