//! User-facing prefab operators. Each function mutates world state
//! directly; UI hookups route to these via the operator system.

use crate::prefab::cache::PrefabAstCache;
use bevy::asset::UntypedAssetId;
use bevy::ecs::hierarchy::Children;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::SceneJsnAst;
use jackdaw_jsn::format::{JsnAssets, JsnEntity, JsnHeader, JsnMetadata, JsnScene};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

/// Write the entity (+ descendants) to a new prefab file at `target_path`.
/// Tags the file's root with `Prefab` and `PrefabEntityId(0)`; descendants
/// get sequential `PrefabEntityId` values. Caches the new prefab. Mutates
/// the live `SceneJsnAst` so `source_root` becomes an `IsA` instance
/// pointing at the new file.
pub fn save_as_prefab(world: &mut World, source_root: Entity, target_path: &Path) {
    let mut entities = vec![source_root];
    collect_descendants(world, source_root, &mut entities);

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let parent_path: PathBuf = target_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let inline_assets: HashMap<UntypedAssetId, String> = HashMap::new();
    let mut jsn_entities: Vec<JsnEntity> = crate::scene_io::build_scene_snapshot(
        world,
        &registry_guard,
        &parent_path,
        &inline_assets,
        &entities,
    );
    drop(registry_guard);

    if let Some(root_entry) = jsn_entities.get_mut(0) {
        root_entry
            .components
            .insert(PREFAB_TYPE.to_string(), serde_json::Value::Null);
        root_entry
            .components
            .insert(PREFAB_ENTITY_ID_TYPE.to_string(), serde_json::json!(0));
        root_entry.parent = None;
    }
    for (i, entry) in jsn_entities.iter_mut().enumerate().skip(1) {
        entry.components.insert(
            PREFAB_ENTITY_ID_TYPE.to_string(),
            serde_json::json!(i as u32),
        );
    }

    let prefab_jsn = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        assets: JsnAssets::default(),
        editor: None,
        scene: jsn_entities,
    };
    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = match serde_json::to_string_pretty(&prefab_jsn) {
        Ok(t) => t,
        Err(err) => {
            warn!("save_as_prefab: failed to serialize prefab: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(target_path, text) {
        warn!(
            "save_as_prefab: failed to write {}: {err}",
            target_path.display()
        );
        return;
    }

    {
        let prefab_ast = SceneJsnAst::from_jsn_scene(&prefab_jsn, &[]);
        let mut cache = world.resource_mut::<PrefabAstCache>();
        cache.insert(target_path, prefab_ast);
    }

    let mut ast = world.resource_mut::<SceneJsnAst>();
    let source_key = match ast.key_for_entity(source_root) {
        Some(k) => k,
        None => {
            let key = ast.add_root();
            if let Some(node) = ast.nodes.get_mut(key) {
                node.ecs_entity = Some(source_root);
            }
            ast.ecs_to_jsn.insert(source_root, key);
            key
        }
    };
    ast.insert_component(
        source_key,
        ISA_TYPE,
        serde_json::json!({
            "source": target_path.to_string_lossy(),
            "deleted": []
        }),
    );
    ast.insert_component(source_key, PREFAB_ENTITY_ID_TYPE, serde_json::json!(0));
    // Tag each authored descendant with the matching `PrefabEntityId` so
    // the resolver recognises them as already-materialised members of
    // the instance; without this it would spawn duplicates from the
    // prefab file. `entities[0]` is `source_root` (already tagged), so
    // descendants start at index 1 and the ids line up with the prefab
    // file's own assignment.
    for (i, descendant_entity) in entities.iter().enumerate().skip(1) {
        if let Some(key) = ast.key_for_entity(*descendant_entity) {
            ast.insert_component(key, PREFAB_ENTITY_ID_TYPE, serde_json::json!(i as u32));
        } else {
            warn!(
                "save_as_prefab: descendant entity {:?} has no AST node; \
                 resolver will spawn a duplicate from the prefab file.",
                descendant_entity
            );
        }
    }
}

fn collect_descendants(world: &World, root: Entity, out: &mut Vec<Entity>) {
    let Some(children) = world.get::<Children>(root) else {
        return;
    };
    for child in children.iter() {
        // Skip editor-internal entities (brush render meshes, gizmos,
        // collider previews, etc). Mirrors the same filter used by the
        // scene save path in `scene_io::collect_scene_entities_from_set`.
        if world.get::<crate::EditorHidden>(child).is_some()
            || world.get::<crate::NonSerializable>(child).is_some()
            || world.get::<crate::SkipSerialization>(child).is_some()
        {
            continue;
        }
        out.push(child);
        collect_descendants(world, child, out);
    }
}

/// Drop any selection entry whose ancestor is also in the input set.
/// De-duplicates while preserving the first appearance's ordering so
/// the caller can assign stable `PrefabEntityId` values to the
/// surviving roots.
fn normalize_selection_roots(world: &World, roots: &[Entity]) -> Vec<Entity> {
    use bevy::ecs::hierarchy::ChildOf;
    use std::collections::HashSet;
    let set: HashSet<Entity> = roots.iter().copied().collect();
    let mut seen: HashSet<Entity> = HashSet::new();
    let mut out: Vec<Entity> = Vec::new();
    for &entity in roots {
        if !seen.insert(entity) {
            continue;
        }
        let mut current = entity;
        let mut covered = false;
        while let Some(ChildOf(parent)) = world.get::<ChildOf>(current) {
            if set.contains(parent) {
                covered = true;
                break;
            }
            current = *parent;
        }
        if !covered {
            out.push(entity);
        }
    }
    out
}

/// Save the given roots (and their descendants) as a single prefab file.
///
/// Selection normalization runs first: any entity whose ancestor is also
/// in `roots` gets dropped (its parent already covers it). The remaining
/// "top roots" are the ones that get packaged.
///
/// - 1 top root: the entity itself becomes the prefab's `PrefabEntityId(0)`
///   and its descendants get sequential ids. Source-scene AST is mutated
///   so the entity becomes an `IsA` instance pointing at the new file,
///   same behaviour as the existing single-root `save_as_prefab`.
/// - More than 1 top root: a synthetic prefab root is created (`Name`
///   `"prefab"`, identity `Transform`, `Prefab` marker, `PrefabEntityId(0)`).
///   Each top root plus its descendants is appended as a child subtree.
///   The source scene is NOT mutated; the user keeps their original
///   entities. If they want an instance they can drag one in from the
///   asset browser.
pub fn save_as_prefab_from_selection(world: &mut World, roots: &[Entity], target_path: &Path) {
    let normalized = normalize_selection_roots(world, roots);
    if normalized.is_empty() {
        warn!("save_as_prefab_from_selection: empty selection");
        return;
    }
    if normalized.len() == 1 {
        save_as_prefab(world, normalized[0], target_path);
        return;
    }

    // BFS each top root in input order so synthetic `PrefabEntityId`
    // assignment 1..N is stable across runs.
    let mut entities: Vec<Entity> = Vec::new();
    for &root in &normalized {
        entities.push(root);
        collect_descendants(world, root, &mut entities);
    }

    let top_root_set: std::collections::HashSet<Entity> = normalized.iter().copied().collect();

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let parent_path: PathBuf = target_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let inline_assets: HashMap<UntypedAssetId, String> = HashMap::new();
    let snapshot = crate::scene_io::build_scene_snapshot(
        world,
        &registry_guard,
        &parent_path,
        &inline_assets,
        &entities,
    );
    drop(registry_guard);

    // Build the synthetic root entry and stitch the snapshot under it.
    let synthetic_components = {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        map.insert(PREFAB_TYPE.to_string(), serde_json::Value::Null);
        map.insert(PREFAB_ENTITY_ID_TYPE.to_string(), serde_json::json!(0));
        map.insert(
            "bevy_ecs::name::Name".to_string(),
            serde_json::Value::String("prefab".to_string()),
        );
        map.insert(
            "bevy_transform::components::transform::Transform".to_string(),
            serde_json::json!({
                "translation": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
        map
    };
    let synthetic_entry = JsnEntity {
        parent: None,
        components: synthetic_components,
    };

    let mut jsn_entities: Vec<JsnEntity> = Vec::with_capacity(snapshot.len() + 1);
    jsn_entities.push(synthetic_entry);
    for (i, mut entry) in snapshot.into_iter().enumerate() {
        let entity = entities[i];
        entry.parent = match entry.parent {
            Some(idx) => Some(idx + 1),
            None => {
                if top_root_set.contains(&entity) {
                    Some(0)
                } else {
                    // Should not happen since every entry except a top
                    // root has its parent inside the entity slice.
                    None
                }
            }
        };
        let next_index = jsn_entities.len() as u32;
        entry.components.insert(
            PREFAB_ENTITY_ID_TYPE.to_string(),
            serde_json::json!(next_index),
        );
        jsn_entities.push(entry);
    }

    let prefab_jsn = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        assets: JsnAssets::default(),
        editor: None,
        scene: jsn_entities,
    };
    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = match serde_json::to_string_pretty(&prefab_jsn) {
        Ok(t) => t,
        Err(err) => {
            warn!("save_as_prefab_from_selection: serialize failed: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(target_path, text) {
        warn!(
            "save_as_prefab_from_selection: failed to write {}: {err}",
            target_path.display()
        );
        return;
    }

    let prefab_ast = SceneJsnAst::from_jsn_scene(&prefab_jsn, &[]);
    world
        .resource_mut::<PrefabAstCache>()
        .insert(target_path, prefab_ast);
}

/// Add a new prefab instance to the live scene at `world_pos`. Caches
/// the prefab AST if missing, mutates the live `SceneJsnAst` to add an
/// instance root carrying `IsA + PrefabEntityId + Transform`, then
/// resolves + respawns the scene preview.
pub fn spawn_instance(world: &mut World, prefab_path: &Path, world_pos: bevy::math::Vec3) {
    let already_cached = world
        .resource::<PrefabAstCache>()
        .get(prefab_path)
        .is_some();
    if !already_cached {
        let Ok(text) = std::fs::read_to_string(prefab_path) else {
            warn!(
                "spawn_instance: failed to read prefab {}",
                prefab_path.display()
            );
            return;
        };
        let scene = match serde_json::from_str::<JsnScene>(&text) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "spawn_instance: failed to parse prefab {}: {e}",
                    prefab_path.display()
                );
                return;
            }
        };
        world
            .resource_mut::<PrefabAstCache>()
            .insert(prefab_path, SceneJsnAst::from_jsn_scene(&scene, &[]));
    }

    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        let key = ast.add_root();
        ast.insert_component(
            key,
            ISA_TYPE,
            serde_json::json!({
                "source": prefab_path.to_string_lossy(),
                "deleted": []
            }),
        );
        ast.insert_component(key, PREFAB_ENTITY_ID_TYPE, serde_json::json!(0));
        // Sparse delta: only the field this caller wants to override.
        // The resolver merges this onto the prefab root's inherited
        // Transform via `apply_deltas`, so the missing `rotation` /
        // `scale` come from the prefab.
        ast.insert_component(
            key,
            "bevy_transform::components::transform::Transform",
            serde_json::json!({
                "translation": [world_pos.x, world_pos.y, world_pos.z],
            }),
        );
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Revert one component field on a prefab instance entity to its
/// inherited value. If the field's current value already equals the
/// prefab's, this is a no-op. After mutating the AST, runs the resolver
/// plus respawns so the live preview reflects the revert.
pub fn revert_field(world: &mut World, entity_key: usize, type_path: &str, field_path: &str) {
    let Some(prefab_value) = resolve_prefab_value(world, entity_key, type_path) else {
        return;
    };
    let Some(prefab_leaf) = walk_dot_path_owned(prefab_value, field_path) else {
        return;
    };

    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        let Some(current) = ast.get_component_at(entity_key, type_path).cloned() else {
            return;
        };
        let next = set_at_path(current, field_path, prefab_leaf);
        ast.replace_component(entity_key, type_path, next);
    }
    crate::prefab::watcher::reload_all_instances(world);
}

/// Revert an entire component on a prefab instance entity to the prefab's
/// value. Bails if the entity has no resolvable prefab inheritance for the
/// component; removing in that case would silently destroy authored data
/// (e.g. when the `IsA` marker has been stripped by an earlier bug).
pub fn revert_component(world: &mut World, entity_key: usize, type_path: &str) {
    let Some(prefab_value) = resolve_prefab_value(world, entity_key, type_path) else {
        warn!(
            "revert_component: no prefab inheritance for entity_key={entity_key} \
             type_path={type_path}; refusing to remove the component"
        );
        return;
    };
    world
        .resource_mut::<SceneJsnAst>()
        .replace_component(entity_key, type_path, prefab_value);
    crate::prefab::watcher::reload_all_instances(world);
}

/// Revert every override on an instance subtree. Walks all descendants
/// of `instance_root_key` (and the root itself); for each that has a
/// `PrefabEntityId`, clears all non-marker components to match the prefab.
pub fn revert_all(world: &mut World, instance_root_key: usize) {
    let mut targets: Vec<(usize, PathBuf, u32)> = Vec::new();
    {
        let ast = world.resource::<SceneJsnAst>();
        let isa = ast.get_component_at(instance_root_key, ISA_TYPE);
        let Some(source) = isa
            .and_then(|v| v.get("source"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
        else {
            return;
        };
        let mut keys = vec![instance_root_key];
        keys.extend(ast.descendants_of(instance_root_key));
        for k in keys {
            let id = ast
                .get_component_at(k, PREFAB_ENTITY_ID_TYPE)
                .and_then(serde_json::Value::as_u64)
                .map(|u| u as u32);
            if let Some(id) = id {
                targets.push((k, source.clone(), id));
            }
        }
    }

    {
        let cache = world.resource::<PrefabAstCache>().clone();
        let mut ast = world.resource_mut::<SceneJsnAst>();
        for (key, prefab_path, prefab_entity_id) in targets {
            let Some(prefab_ast) = cache.get(&prefab_path) else {
                continue;
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
                continue;
            };
            let Some(prefab_components) = prefab_ast.components_at(prefab_key) else {
                continue;
            };

            if let Some(node) = ast.nodes.get_mut(key) {
                let preserved_isa = node.components.get(ISA_TYPE).cloned();
                let preserved_id = node.components.get(PREFAB_ENTITY_ID_TYPE).cloned();
                node.components.clear();
                for (k, v) in prefab_components {
                    if k == PREFAB_TYPE {
                        continue;
                    }
                    node.components.insert(k.clone(), v.clone());
                }
                if let Some(isa) = preserved_isa {
                    node.components.insert(ISA_TYPE.to_string(), isa);
                }
                if let Some(id) = preserved_id {
                    node.components
                        .insert(PREFAB_ENTITY_ID_TYPE.to_string(), id);
                }
            }
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

fn resolve_prefab_value(
    world: &World,
    entity_key: usize,
    type_path: &str,
) -> Option<serde_json::Value> {
    let ast = world.resource::<SceneJsnAst>();
    let cache = world.resource::<PrefabAstCache>();
    let prefab_entity_id = ast
        .get_component_at(entity_key, PREFAB_ENTITY_ID_TYPE)
        .and_then(serde_json::Value::as_u64)
        .map(|u| u as u32)?;
    let mut current = entity_key;
    let prefab_path = loop {
        if let Some(isa) = ast.get_component_at(current, ISA_TYPE) {
            let source = isa.get("source").and_then(|v| v.as_str())?;
            break PathBuf::from(source);
        }
        current = ast.nodes.get(current)?.parent?;
    };
    let prefab_ast = cache.get(&prefab_path)?;
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
    })?;
    prefab_ast.get_component_at(prefab_key, type_path).cloned()
}

fn walk_dot_path_owned(value: serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut cursor = value;
    for part in path.split('.') {
        let serde_json::Value::Object(mut map) = cursor else {
            return None;
        };
        cursor = map.remove(part)?;
    }
    Some(cursor)
}

fn set_at_path(base: serde_json::Value, path: &str, leaf: serde_json::Value) -> serde_json::Value {
    fn rec(
        cursor: serde_json::Value,
        parts: &[&str],
        leaf: serde_json::Value,
    ) -> serde_json::Value {
        if parts.is_empty() {
            return leaf;
        }
        let mut map = match cursor {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        let key = parts[0];
        let next = map.remove(key).unwrap_or(serde_json::Value::Null);
        let replaced = rec(next, &parts[1..], leaf);
        map.insert(key.to_string(), replaced);
        serde_json::Value::Object(map)
    }
    let parts: Vec<&str> = path.split('.').collect();
    rec(base, &parts, leaf)
}

/// Convert an existing prefab instance into a new variant prefab. The
/// new file has its own `Prefab` marker AND `IsA` referencing the
/// original prefab, so it inherits from the original while carrying
/// the instance's current overrides as its own base. The source scene's
/// instance is rewired to point at the variant.
pub fn save_as_variant(world: &mut World, instance_root: Entity, target_path: &Path) {
    let (instance_key, isa, instance_components, descendant_data) = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(instance_key) = ast.key_for_entity(instance_root) else {
            warn!("save_as_variant: instance entity not in AST");
            return;
        };
        let Some(isa) = ast.get_component_at(instance_key, ISA_TYPE).cloned() else {
            warn!("save_as_variant: instance lacks IsA");
            return;
        };
        let instance_components: Vec<(String, serde_json::Value)> = ast
            .components_at(instance_key)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let descendant_data: Vec<(u32, Vec<(String, serde_json::Value)>)> = ast
            .descendants_of(instance_key)
            .into_iter()
            .filter_map(|child_key| {
                let id = ast
                    .get_component_at(child_key, PREFAB_ENTITY_ID_TYPE)
                    .and_then(serde_json::Value::as_u64)? as u32;
                let components: Vec<(String, serde_json::Value)> = ast
                    .components_at(child_key)
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                Some((id, components))
            })
            .collect();
        (instance_key, isa, instance_components, descendant_data)
    };

    // Build the variant AST.
    let mut variant_ast = SceneJsnAst::default();
    let variant_root = variant_ast.add_root();
    variant_ast.insert_component(variant_root, PREFAB_TYPE, serde_json::Value::Null);
    variant_ast.insert_component(variant_root, PREFAB_ENTITY_ID_TYPE, serde_json::json!(0));
    variant_ast.insert_component(variant_root, ISA_TYPE, isa.clone());
    for (type_path, value) in &instance_components {
        if matches!(type_path.as_str(), s if s == ISA_TYPE || s == PREFAB_TYPE || s == PREFAB_ENTITY_ID_TYPE)
        {
            continue;
        }
        variant_ast.insert_component(variant_root, type_path, value.clone());
    }
    for (id, components) in &descendant_data {
        let new_child = variant_ast.add_child(variant_root);
        variant_ast.insert_component(new_child, PREFAB_ENTITY_ID_TYPE, serde_json::json!(*id));
        for (type_path, value) in components {
            if type_path == PREFAB_ENTITY_ID_TYPE {
                continue;
            }
            variant_ast.insert_component(new_child, type_path, value.clone());
        }
    }

    // Write to disk.
    let variant_jsn = crate::scene_io::jsn_scene_from_ast(&variant_ast);
    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = match serde_json::to_string_pretty(&variant_jsn) {
        Ok(t) => t,
        Err(err) => {
            warn!("save_as_variant: serialize failed: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(target_path, text) {
        warn!(
            "save_as_variant: failed to write {}: {err}",
            target_path.display()
        );
        return;
    }

    // Cache the variant.
    world
        .resource_mut::<PrefabAstCache>()
        .insert(target_path, variant_ast);

    // Rewire the source scene's instance to point at the variant and
    // clear its now-redundant overrides on the descendants (they live
    // in the variant's base now).
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        ast.replace_component(
            instance_key,
            ISA_TYPE,
            serde_json::json!({
                "source": target_path.to_string_lossy(),
                "deleted": []
            }),
        );
        let descendant_keys = ast.descendants_of(instance_key);
        for child_key in descendant_keys {
            let component_paths: Vec<String> = ast
                .components_at(child_key)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            for type_path in component_paths {
                if type_path == PREFAB_ENTITY_ID_TYPE {
                    continue;
                }
                ast.remove_component(child_key, &type_path);
            }
        }
    }
}

/// Apply a single-field delta to every prefab instance in the scene
/// that points at `source_path`. The delta path can be dotted (e.g.
/// `"scale.x"`) and follows the same semantics as `apply_deltas`.
pub fn bulk_apply_in_scene(
    world: &mut World,
    source_path: &Path,
    type_path: &str,
    field_path: &str,
    value: serde_json::Value,
) {
    let source = source_path.to_string_lossy().to_string();
    let matches: Vec<usize> = {
        let ast = world.resource::<SceneJsnAst>();
        ast.entities_with_component(ISA_TYPE)
            .filter(|k| {
                ast.get_component_at(*k, ISA_TYPE)
                    .and_then(|v| v.get("source"))
                    .and_then(|v| v.as_str())
                    .map(|s| s == source)
                    .unwrap_or(false)
            })
            .collect()
    };
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        for key in matches {
            let mut current = ast
                .get_component_at(key, type_path)
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            if let Err(err) = crate::prefab::overrides::apply_deltas(
                &mut current,
                &serde_json::json!({ field_path: value.clone() }),
            ) {
                warn!("bulk_apply_in_scene: apply_deltas failed: {err:?}");
                continue;
            }
            ast.replace_component(key, type_path, current);
        }
    }
    crate::prefab::watcher::reload_all_instances(world);
}

/// Apply a single-field value into a prefab's source AST so the override
/// becomes the new inherited base. Mutates the cache in place; the
/// resolve-on-change driver picks up the epoch bump and respawns the
/// active scene next frame. Also clears the matching delta from the
/// source instance so it inherits the new value cleanly. No disk write
/// here; persistence is a separate explicit save step.
pub fn apply_to_prefab_source(
    world: &mut World,
    instance_root: usize,
    entity_id: u32,
    type_path: &str,
    field_path: &str,
    value: serde_json::Value,
) {
    let source_path: PathBuf = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(isa) = ast.get_component_at(instance_root, ISA_TYPE) else {
            warn!("apply_to_prefab_source: instance lacks IsA");
            return;
        };
        let Some(source) = isa.get("source").and_then(|v| v.as_str()) else {
            warn!("apply_to_prefab_source: IsA.source missing");
            return;
        };
        PathBuf::from(source)
    };

    let applied = {
        let mut cache = world.resource_mut::<PrefabAstCache>();
        cache.mutate(&source_path, |prefab_ast| {
            let Some(target_key) = prefab_ast
                .entities_with_component(PREFAB_ENTITY_ID_TYPE)
                .find(|k| {
                    prefab_ast
                        .get_component_at(*k, PREFAB_ENTITY_ID_TYPE)
                        .and_then(serde_json::Value::as_u64)
                        == Some(entity_id as u64)
                })
            else {
                warn!("apply_to_prefab_source: PrefabEntityId({entity_id}) not in prefab");
                return;
            };
            let mut current = prefab_ast
                .get_component_at(target_key, type_path)
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            if let Err(err) = crate::prefab::overrides::apply_deltas(
                &mut current,
                &serde_json::json!({ field_path: value.clone() }),
            ) {
                warn!("apply_to_prefab_source: apply_deltas failed: {err:?}");
                return;
            }
            prefab_ast.replace_component(target_key, type_path, current);
        })
    };
    if !applied {
        warn!(
            "apply_to_prefab_source: prefab not cached: {}",
            source_path.display()
        );
        return;
    }

    // Clear the matching delta on the source-scene side.
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        let mut candidates = ast.descendants_of(instance_root);
        candidates.push(instance_root);
        let scene_key = candidates.into_iter().find(|k| {
            ast.get_component_at(*k, PREFAB_ENTITY_ID_TYPE)
                .and_then(serde_json::Value::as_u64)
                == Some(entity_id as u64)
        });
        if let Some(scene_key) = scene_key
            && let Some(current) = ast.get_component_at(scene_key, type_path).cloned()
            && let serde_json::Value::Object(mut map) = current
        {
            map.remove(field_path);
            if map.is_empty() {
                ast.remove_component(scene_key, type_path);
            } else {
                ast.replace_component(scene_key, type_path, serde_json::Value::Object(map));
            }
        }
    }
}

/// Remove an inherited child from its prefab instance and re-parent it
/// under `drop_target_key` as a standalone (non-instance) entity. The
/// child's flattened component data is copied (the resolver's merge
/// result), and the parent instance's `IsA.deleted` list is extended
/// with the child's `PrefabEntityId` so the resolver no longer
/// re-materializes it on future loads.
pub fn unpack_child(world: &mut World, child_key: usize, drop_target_key: usize) {
    let mut ast = world.resource_mut::<SceneJsnAst>();

    let Some(id) = ast
        .get_component_at(child_key, PREFAB_ENTITY_ID_TYPE)
        .and_then(serde_json::Value::as_u64)
    else {
        warn!("unpack_child: child lacks PrefabEntityId");
        return;
    };
    let id = id as u32;

    let Some(instance_root) = ast.ancestor_with_component(child_key, ISA_TYPE) else {
        warn!("unpack_child: child has no IsA ancestor");
        return;
    };

    // Update IsA.deleted on the instance root.
    let mut isa = ast
        .get_component_at(instance_root, ISA_TYPE)
        .cloned()
        .unwrap_or(serde_json::json!({ "source": "", "deleted": [] }));
    if let Some(deleted) = isa.get_mut("deleted").and_then(|v| v.as_array_mut())
        && !deleted.iter().any(|v| v.as_u64() == Some(id as u64))
    {
        deleted.push(serde_json::json!(id));
    }
    ast.replace_component(instance_root, ISA_TYPE, isa);

    // Copy the child's flattened components under the drop target as a
    // standalone child.
    let new_key = ast.add_child(drop_target_key);
    let component_pairs: Vec<(String, serde_json::Value)> = ast
        .components_at(child_key)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    for (type_path, value) in component_pairs {
        if type_path == PREFAB_ENTITY_ID_TYPE {
            continue;
        }
        ast.insert_component(new_key, &type_path, value);
    }
}

/// Walk every entity in the prefab-instance subtree rooted at
/// `instance_root_key`. For each one that carries a `PrefabEntityId`,
/// diff its non-marker components against the cached prefab's matching
/// entity and call `apply_to_prefab_source` for every overridden leaf.
/// At the end, the prefab source file holds all the user's edits and
/// the instance has no remaining overrides.
pub fn apply_all_overrides_to_source(world: &mut World, instance_root_key: usize) {
    // Resolve the instance's prefab source path.
    let prefab_path: PathBuf = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(isa) = ast.get_component_at(instance_root_key, ISA_TYPE) else {
            return;
        };
        let Some(source) = isa.get("source").and_then(|v| v.as_str()) else {
            return;
        };
        PathBuf::from(source)
    };

    // Gather every (entity_key, prefab_entity_id) pair under the instance.
    let pairs: Vec<(usize, u32)> = {
        let ast = world.resource::<SceneJsnAst>();
        let mut out = Vec::new();
        let id = ast
            .get_component_at(instance_root_key, PREFAB_ENTITY_ID_TYPE)
            .and_then(serde_json::Value::as_u64)
            .map(|u| u as u32);
        if let Some(id) = id {
            out.push((instance_root_key, id));
        }
        for descendant_key in ast.descendants_of(instance_root_key) {
            if let Some(id) = ast
                .get_component_at(descendant_key, PREFAB_ENTITY_ID_TYPE)
                .and_then(serde_json::Value::as_u64)
                .map(|u| u as u32)
            {
                out.push((descendant_key, id));
            }
        }
        out
    };

    // For each entity, walk its components and apply each overridden
    // leaf to the prefab source.
    for (entity_key, prefab_entity_id) in pairs {
        let component_overrides: Vec<(String, Vec<(String, serde_json::Value)>)> = {
            let ast = world.resource::<SceneJsnAst>();
            let cache = world.resource::<PrefabAstCache>();
            let Some(prefab_ast) = cache.get(&prefab_path) else {
                continue;
            };
            let Some(prefab_match) = prefab_ast.nodes.iter().enumerate().find_map(|(i, n)| {
                let id = n
                    .components
                    .get(PREFAB_ENTITY_ID_TYPE)
                    .and_then(serde_json::Value::as_u64)?;
                (id as u32 == prefab_entity_id).then_some(i)
            }) else {
                continue;
            };
            let Some(components) = ast.components_at(entity_key) else {
                continue;
            };
            components
                .iter()
                .filter(|(type_path, _)| {
                    type_path.as_str() != PREFAB_TYPE
                        && type_path.as_str() != ISA_TYPE
                        && type_path.as_str() != PREFAB_ENTITY_ID_TYPE
                })
                .map(|(type_path, scene_value)| {
                    let prefab_value = prefab_ast.get_component_at(prefab_match, type_path);
                    let leaves = collect_overridden_leaves(scene_value, prefab_value);
                    (type_path.clone(), leaves)
                })
                .filter(|(_, leaves)| !leaves.is_empty())
                .collect()
        };

        // The instance_root_key for `apply_to_prefab_source` must always
        // be the prefab instance's root, not the descendant entity_key.
        for (type_path, leaves) in component_overrides {
            for (field_path, value) in leaves {
                apply_to_prefab_source(
                    world,
                    instance_root_key,
                    prefab_entity_id,
                    &type_path,
                    &field_path,
                    value,
                );
            }
        }
    }
}

/// Returns `(dot_path, leaf)` pairs for every scalar / non-object leaf
/// in `scene` that differs from `prefab`'s value at the same path.
/// Mirrors the `collect_overridden_paths` helper in
/// `src/inspector/prefab_menu.rs` but exposed for reuse.
fn collect_overridden_leaves(
    scene: &serde_json::Value,
    prefab: Option<&serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    walk_leaves(scene, prefab, String::new(), &mut out);
    out
}

fn walk_leaves(
    scene: &serde_json::Value,
    prefab: Option<&serde_json::Value>,
    path: String,
    out: &mut Vec<(String, serde_json::Value)>,
) {
    match scene {
        serde_json::Value::Object(scene_map) => {
            let prefab_map = prefab.and_then(serde_json::Value::as_object);
            for (key, child) in scene_map {
                let next_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk_leaves(child, prefab_map.and_then(|m| m.get(key)), next_path, out);
            }
        }
        leaf => {
            let differs = match prefab {
                Some(p) => p != leaf,
                None => true,
            };
            if differs && !path.is_empty() {
                out.push((path, leaf.clone()));
            }
        }
    }
}

/// Write a prefab's cached AST to disk and record the saved
/// fingerprint so the file watcher ignores its own echo.
pub fn save_prefab_to_disk(world: &mut World, prefab_path: &Path) -> std::io::Result<()> {
    let prefab_jsn = {
        let cache = world.resource::<PrefabAstCache>();
        let Some(ast) = cache.get(prefab_path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("prefab not cached: {}", prefab_path.display()),
            ));
        };
        crate::scene_io::jsn_scene_from_ast(ast)
    };
    let text = serde_json::to_string_pretty(&prefab_jsn).map_err(std::io::Error::other)?;
    std::fs::write(prefab_path, text)?;

    let fingerprint = crate::prefab::cache::compute_file_fingerprint(prefab_path)?;
    world
        .resource_mut::<PrefabAstCache>()
        .record_saved_fingerprint(prefab_path, fingerprint);
    Ok(())
}

/// Save the active prefab tab's cached AST to its source file.
#[operator(
    id = "prefab.save",
    label = "Save Prefab",
    description = "Write the active prefab tab's AST out to its source file.",
    allows_undo = false
)]
pub fn prefab_save(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let active_path = {
            let scenes = world.resource::<crate::scenes::Scenes>();
            match scenes.tabs.get(scenes.active).map(|t| &t.content) {
                Some(crate::scenes::TabContent::Prefab(path)) => Some(path.as_path().to_path_buf()),
                _ => None,
            }
        };
        let Some(path) = active_path else {
            warn!("prefab.save: active tab is not a prefab");
            return;
        };
        if let Err(err) = save_prefab_to_disk(world, path.as_path()) {
            warn!("prefab.save: write failed: {err}");
        }
    });
    OperatorResult::Finished
}

/// Spawn a new prefab instance at a world-space position. Reads `path`
/// (the prefab `.jsn` to instantiate) plus `pos_x`, `pos_y`, `pos_z`
/// (the spawn position).
#[operator(
    id = "prefab.spawn_instance",
    label = "Spawn Prefab Instance",
    description = "Drop a new instance of the given prefab into the active scene at a world position.",
    allows_undo = false,
    params(
        path(String, doc = "Path to the prefab `.jsn` to instantiate."),
        pos_x(f64, doc = "World-space X position."),
        pos_y(f64, doc = "World-space Y position."),
        pos_z(f64, doc = "World-space Z position."),
    )
)]
pub fn prefab_spawn_instance(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_string) else {
        warn!("prefab.spawn_instance: missing `path` param");
        return OperatorResult::Cancelled;
    };
    let Some(x) = params.as_float("pos_x") else {
        warn!("prefab.spawn_instance: missing `pos_x` param");
        return OperatorResult::Cancelled;
    };
    let Some(y) = params.as_float("pos_y") else {
        warn!("prefab.spawn_instance: missing `pos_y` param");
        return OperatorResult::Cancelled;
    };
    let Some(z) = params.as_float("pos_z") else {
        warn!("prefab.spawn_instance: missing `pos_z` param");
        return OperatorResult::Cancelled;
    };
    let pos = bevy::math::Vec3::new(x as f32, y as f32, z as f32);
    commands.queue(move |world: &mut World| {
        spawn_instance(world, &PathBuf::from(path), pos);
    });
    OperatorResult::Finished
}

/// Revert a single component field on a prefab-instance entity back to
/// its inherited prefab value.
#[operator(
    id = "prefab.revert_field",
    label = "Revert Field to Prefab",
    description = "Restore one field on a prefab-instance entity to its inherited prefab value.",
    allows_undo = false,
    params(
        entity_key(i64, doc = "AST key of the instance entity."),
        type_path(String, doc = "Fully-qualified component type path."),
        field_path(String, doc = "Dotted field path within the component."),
    )
)]
pub fn prefab_revert_field(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity_key) = params.as_int("entity_key") else {
        warn!("prefab.revert_field: missing `entity_key` param");
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        warn!("prefab.revert_field: missing `type_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = params.as_str("field_path").map(str::to_string) else {
        warn!("prefab.revert_field: missing `field_path` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        revert_field(world, entity_key as usize, &type_path, &field_path);
    });
    OperatorResult::Finished
}

/// Revert an entire component on a prefab-instance entity back to the
/// prefab's inherited value.
#[operator(
    id = "prefab.revert_component",
    label = "Revert Component to Prefab",
    description = "Restore the component on a prefab-instance entity to its inherited prefab value.",
    allows_undo = false,
    params(
        entity_key(i64, doc = "AST key of the instance entity."),
        type_path(String, doc = "Fully-qualified component type path."),
    )
)]
pub fn prefab_revert_component(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity_key) = params.as_int("entity_key") else {
        warn!("prefab.revert_component: missing `entity_key` param");
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        warn!("prefab.revert_component: missing `type_path` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        revert_component(world, entity_key as usize, &type_path);
    });
    OperatorResult::Finished
}

/// Revert every override on a prefab-instance subtree.
#[operator(
    id = "prefab.revert_all",
    label = "Revert All Overrides",
    description = "Remove every per-instance override on a prefab-instance subtree.",
    allows_undo = false,
    params(instance_root(i64, doc = "AST key of the instance root."),)
)]
pub fn prefab_revert_all(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(instance_root) = params.as_int("instance_root") else {
        warn!("prefab.revert_all: missing `instance_root` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        revert_all(world, instance_root as usize);
    });
    OperatorResult::Finished
}

/// Apply a single field's scene-side value into the prefab source AST so
/// the override becomes the new inherited base. The new value is
/// supplied as a JSON-encoded string via the `value_json` param.
#[operator(
    id = "prefab.apply_to_source",
    label = "Apply Field to Prefab Source",
    description = "Push one overridden field into the prefab source so every instance picks it up.",
    allows_undo = false,
    params(
        instance_root(i64, doc = "AST key of the prefab-instance root."),
        entity_id(i64, doc = "PrefabEntityId of the target entity inside the prefab."),
        type_path(String, doc = "Fully-qualified component type path."),
        field_path(String, doc = "Dotted field path within the component."),
        value_json(String, doc = "JSON-encoded field value to apply."),
    )
)]
pub fn prefab_apply_to_source(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(instance_root) = params.as_int("instance_root") else {
        warn!("prefab.apply_to_source: missing `instance_root` param");
        return OperatorResult::Cancelled;
    };
    let Some(entity_id) = params.as_int("entity_id") else {
        warn!("prefab.apply_to_source: missing `entity_id` param");
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        warn!("prefab.apply_to_source: missing `type_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = params.as_str("field_path").map(str::to_string) else {
        warn!("prefab.apply_to_source: missing `field_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(value_json) = params.as_str("value_json").map(str::to_string) else {
        warn!("prefab.apply_to_source: missing `value_json` param");
        return OperatorResult::Cancelled;
    };
    let value: serde_json::Value = match serde_json::from_str(&value_json) {
        Ok(v) => v,
        Err(err) => {
            warn!("prefab.apply_to_source: bad `value_json`: {err}");
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| {
        apply_to_prefab_source(
            world,
            instance_root as usize,
            entity_id as u32,
            &type_path,
            &field_path,
            value,
        );
    });
    OperatorResult::Finished
}

/// Apply a single-field delta to every prefab instance in the scene
/// that points at `source_path`.
#[operator(
    id = "prefab.bulk_apply_in_scene",
    label = "Bulk Apply Field in Scene",
    description = "Copy one overridden field to every other prefab instance in the scene that shares the same source.",
    allows_undo = false,
    params(
        source_path(String, doc = "Prefab source path to match instances against."),
        type_path(String, doc = "Fully-qualified component type path."),
        field_path(String, doc = "Dotted field path within the component."),
        value_json(String, doc = "JSON-encoded field value to apply."),
    )
)]
pub fn prefab_bulk_apply_in_scene(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(source_path) = params.as_str("source_path").map(str::to_string) else {
        warn!("prefab.bulk_apply_in_scene: missing `source_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        warn!("prefab.bulk_apply_in_scene: missing `type_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = params.as_str("field_path").map(str::to_string) else {
        warn!("prefab.bulk_apply_in_scene: missing `field_path` param");
        return OperatorResult::Cancelled;
    };
    let Some(value_json) = params.as_str("value_json").map(str::to_string) else {
        warn!("prefab.bulk_apply_in_scene: missing `value_json` param");
        return OperatorResult::Cancelled;
    };
    let value: serde_json::Value = match serde_json::from_str(&value_json) {
        Ok(v) => v,
        Err(err) => {
            warn!("prefab.bulk_apply_in_scene: bad `value_json`: {err}");
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| {
        bulk_apply_in_scene(
            world,
            &PathBuf::from(source_path),
            &type_path,
            &field_path,
            value,
        );
    });
    OperatorResult::Finished
}

/// Convert an existing prefab instance into a new variant prefab file.
#[operator(
    id = "prefab.save_as_variant_entity",
    label = "Save Instance as Variant",
    description = "Write a prefab-instance entity out as a new variant prefab file inheriting from the original.",
    allows_undo = false,
    params(
        instance_root_entity(i64, doc = "Bits of the instance-root Entity."),
        target_path(String, doc = "Path to write the new variant file to."),
    )
)]
pub fn prefab_save_as_variant_entity(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(bits) = params.as_int("instance_root_entity") else {
        warn!("prefab.save_as_variant_entity: missing `instance_root_entity` param");
        return OperatorResult::Cancelled;
    };
    let Some(target_path) = params.as_str("target_path").map(str::to_string) else {
        warn!("prefab.save_as_variant_entity: missing `target_path` param");
        return OperatorResult::Cancelled;
    };
    let entity = Entity::from_bits(bits as u64);
    commands.queue(move |world: &mut World| {
        save_as_variant(world, entity, &PathBuf::from(target_path));
    });
    OperatorResult::Finished
}

/// Pop an inherited child out of its prefab instance and re-parent it
/// under another entity in the scene as a standalone (non-instance)
/// entity.
#[operator(
    id = "prefab.unpack_child",
    label = "Unpack Prefab Child",
    description = "Detach an inherited prefab child and re-parent it under another scene entity.",
    allows_undo = false,
    params(
        child_key(i64, doc = "AST key of the inherited child."),
        drop_target_key(i64, doc = "AST key of the entity to re-parent under."),
    )
)]
pub fn prefab_unpack_child(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(child_key) = params.as_int("child_key") else {
        warn!("prefab.unpack_child: missing `child_key` param");
        return OperatorResult::Cancelled;
    };
    let Some(drop_target_key) = params.as_int("drop_target_key") else {
        warn!("prefab.unpack_child: missing `drop_target_key` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        unpack_child(world, child_key as usize, drop_target_key as usize);
    });
    OperatorResult::Finished
}
