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
        // Skip ECS-only derived children (brush clip overlays, face
        // entities, etc.) that have no AST node. Persisting them as
        // top-level prefab entries would orphan them after respawn,
        // because the in-place restructure can't reparent unknown ECS
        // entities back under the brush they belong to. The brush spawn
        // pipeline recreates them from the brush data, so they don't
        // belong in the prefab file at all.
        let ast = world.resource::<jackdaw_jsn::SceneJsnAst>();
        if !ast.ecs_to_jsn.contains_key(&child) {
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

/// Save the given roots (and their descendants) as a prefab file, then
/// replace them in the source scene with a fresh instance.
///
/// Selection normalization runs first: any entity whose ancestor is also
/// in `roots` gets dropped (its parent already covers it). The remaining
/// "top roots" are the ones that get packaged.
///
/// If a selected root carries `IsA`, this is the **propagate** path:
/// the instance's current resolved state (inherited + overrides +
/// local-only children) is written back to the prefab file. The
/// instance entity stays at its current scene position.
///
/// Otherwise this is the **bundle** path: the prefab file is written
/// from the selection snapshot, the selection is removed from the
/// source scene's AST and ECS, and a fresh instance is spawned at the
/// selection's centroid via `spawn_instance`. The resolver then
/// materialises the inherited children — same shape as a drag-spawn
/// instance.
pub fn save_as_prefab_from_selection(world: &mut World, roots: &[Entity], target_path: &Path) {
    let normalized = normalize_selection_roots(world, roots);
    if normalized.is_empty() {
        warn!("save_as_prefab_from_selection: empty selection");
        return;
    }

    // Propagate only when the selection is a single instance root, the
    // target path matches its existing IsA source, AND the prefab is
    // cached (i.e. it actually exists and the resolver can run).
    // Anything else falls through to the bundle path, which strips
    // any stale IsA and writes a fresh prefab.
    let propagate_target = if normalized.len() == 1 {
        let root = normalized[0];
        let ast = world.resource::<SceneJsnAst>();
        let cache = world.resource::<PrefabAstCache>();
        ast.key_for_entity(root).and_then(|key| {
            let isa = ast.get_component_at(key, ISA_TYPE)?;
            let source = isa.get("source").and_then(|v| v.as_str())?;
            if Path::new(source) == target_path && cache.get(target_path).is_some() {
                Some(key)
            } else {
                None
            }
        })
    } else {
        None
    };
    if let Some(instance_key) = propagate_target {
        propagate_instance_to_prefab(world, instance_key, target_path);
        return;
    }

    save_selection_as_new_prefab(world, &normalized, target_path);
}

/// Bundle path: write a fresh prefab file from the snapshot and replace
/// the selection in the source scene with an instance of the new prefab.
fn save_selection_as_new_prefab(world: &mut World, normalized: &[Entity], target_path: &Path) {
    // BFS each top root in input order so `PrefabEntityId` assignment
    // 1..N is stable across runs.
    let mut entities: Vec<Entity> = Vec::new();
    for &root in normalized {
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

    // Centroid of the normalized top-roots becomes the new instance's
    // translation so the wrapper sits at the visual center of the
    // packaged geometry. Read GlobalTransform so a selection under a
    // non-identity parent bundles relative to its real world position;
    // fall back to Transform for top-level entities where the resolver
    // hasn't populated GlobalTransform yet (first frame after spawn).
    let centroid: bevy::math::Vec3 = {
        let mut sum = bevy::math::Vec3::ZERO;
        let mut count = 0u32;
        for &root in normalized {
            if let Some(gt) = world.get::<bevy::prelude::GlobalTransform>(root) {
                sum += gt.translation();
                count += 1;
            } else if let Some(t) = world.get::<bevy::prelude::Transform>(root) {
                sum += t.translation;
                count += 1;
            }
        }
        if count > 0 {
            sum / count as f32
        } else {
            bevy::math::Vec3::ZERO
        }
    };

    if !write_prefab_file(target_path, &snapshot, &entities, &top_root_set, centroid) {
        return;
    }

    // Remove the AST entries for the packaged entities so the upcoming
    // reload_all_instances doesn't respawn them alongside the new
    // instance. The ECS entities will be cleaned up by reload's
    // clear_scene_entities pass.
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        for &entity in &entities {
            ast.remove_node(entity);
        }
    }

    // spawn_instance adds the new instance node to the AST, then
    // triggers reload_all_instances which clears + respawns the world.
    // The resolver materialises the inherited children from the prefab
    // file we just wrote, producing the same shape as a drag-spawn.
    spawn_instance(world, target_path, centroid);
}

/// Write `snapshot` as a prefab file at `target_path`, with the
/// synthetic root carrying `Name = file stem`, identity Transform, and
/// `Visibility::Inherited`. Children's translations are shifted by
/// `-centroid` so a fresh instance spawn reproduces the original world
/// layout. Returns false on write failure.
fn write_prefab_file(
    target_path: &Path,
    snapshot: &[JsnEntity],
    entities: &[Entity],
    top_root_set: &std::collections::HashSet<Entity>,
    centroid: bevy::math::Vec3,
) -> bool {
    // Strip stale prefab markers and shift every entry's parent index by
    // +1 because we'll prepend the synthetic root at index 0.
    let mut prefab_entities: Vec<JsnEntity> = snapshot
        .iter()
        .cloned()
        .map(|mut e| {
            e.components.remove(PREFAB_TYPE);
            e.components.remove(ISA_TYPE);
            e.components.remove(PREFAB_ENTITY_ID_TYPE);
            if let Some(p) = e.parent {
                e.parent = Some(p + 1);
            }
            e
        })
        .collect();

    // Top roots in the snapshot had no parent (their natural parents
    // weren't in the slice). Parent them under the synthetic root and
    // shift their Transform.translation into the synthetic root's
    // local frame.
    for (i, entry) in prefab_entities.iter_mut().enumerate() {
        if entry.parent.is_none() && top_root_set.contains(&entities[i]) {
            entry.parent = Some(0);
            shift_transform_translation(&mut entry.components, -centroid);
        }
    }
    // Sequential PrefabEntityId 1..N for every packaged entity.
    for (i, entry) in prefab_entities.iter_mut().enumerate() {
        entry.components.insert(
            PREFAB_ENTITY_ID_TYPE.to_string(),
            serde_json::json!((i + 1) as u32),
        );
    }

    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PrefabRoot")
        .to_string();
    let synthetic_entry = JsnEntity {
        parent: None,
        components: synthetic_root_components(display_name),
    };

    let mut final_entities: Vec<JsnEntity> = Vec::with_capacity(prefab_entities.len() + 1);
    final_entities.push(synthetic_entry);
    final_entities.extend(prefab_entities);

    let prefab_jsn = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        assets: JsnAssets::default(),
        editor: None,
        scene: final_entities,
    };
    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = match serde_json::to_string_pretty(&prefab_jsn) {
        Ok(t) => t,
        Err(err) => {
            warn!("save_as_prefab_from_selection: serialize failed: {err}");
            return false;
        }
    };
    if let Err(err) = std::fs::write(target_path, &text) {
        warn!(
            "save_as_prefab_from_selection: failed to write {}: {err}",
            target_path.display()
        );
        return false;
    }
    true
}

/// Components for the synthetic prefab root entry at index 0:
/// `Prefab` marker, `PrefabEntityId(0)`, `Name = display_name`,
/// identity `Transform`, and `Visibility::Inherited`. Visibility is
/// required for Bevy's hierarchy propagation (`InheritedVisibility` /
/// `ViewVisibility` / `GlobalTransform`); the prefab needs to carry it
/// so the resolver merges it onto each instance.
fn synthetic_root_components(display_name: String) -> HashMap<String, serde_json::Value> {
    let mut map: HashMap<String, serde_json::Value> = HashMap::new();
    map.insert(PREFAB_TYPE.to_string(), serde_json::Value::Null);
    map.insert(PREFAB_ENTITY_ID_TYPE.to_string(), serde_json::json!(0));
    map.insert(
        "bevy_ecs::name::Name".to_string(),
        serde_json::Value::String(display_name),
    );
    map.insert(
        "bevy_transform::components::transform::Transform".to_string(),
        serde_json::json!({
            "translation": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        }),
    );
    map.insert(
        "bevy_camera::visibility::Visibility".to_string(),
        serde_json::Value::String("Inherited".to_string()),
    );
    map
}

/// Propagate path: snapshot the instance's current resolved subtree
/// (inherited + overrides + local-only) and write it back to the
/// prefab file at `target_path`. The instance entity stays put in the
/// source scene. Other instances of the same prefab pick up the new
/// baseline on the next cache-driven respawn.
fn propagate_instance_to_prefab(world: &mut World, instance_key: usize, target_path: &Path) {
    // Resolve the source AST so the snapshot reflects merged overrides
    // and materialised inherited descendants.
    let resolved = {
        let unresolved = world.resource::<SceneJsnAst>().clone();
        let cache = world.resource::<PrefabAstCache>();
        match crate::prefab::resolver::resolve_scene(&unresolved, cache) {
            Ok(r) => r,
            Err(err) => {
                warn!("propagate_instance_to_prefab: resolver failed: {err}");
                return;
            }
        }
    };

    // Find the resolved index for the instance. Since resolve_scene
    // returns a clone with descendants appended (not inserted), the
    // authored indices line up with the unresolved AST.
    let resolved_instance_idx = instance_key;

    // Walk the resolved AST under the instance to collect descendant
    // indices in BFS order. These become the prefab's child entries.
    let mut descendants: Vec<usize> = Vec::new();
    let mut stack: Vec<usize> = vec![resolved_instance_idx];
    while let Some(idx) = stack.pop() {
        let children: Vec<usize> = resolved.children_of(idx).collect();
        for child in children {
            descendants.push(child);
            stack.push(child);
        }
    }

    // Build the child entries from the resolved AST. Strip prefab
    // markers (they get rewritten with fresh sequential IDs) and remap
    // parent indices to the prefab's index space: instance -> 0,
    // descendants -> 1..N.
    let mut old_to_new: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    old_to_new.insert(resolved_instance_idx, 0);
    for (i, &d) in descendants.iter().enumerate() {
        old_to_new.insert(d, i + 1);
    }

    let mut prefab_entries: Vec<JsnEntity> = Vec::with_capacity(descendants.len());
    for &d in &descendants {
        let Some(resolved_node) = resolved.nodes.get(d) else {
            continue;
        };
        let mut components = resolved_node.components.clone();
        components.remove(PREFAB_TYPE);
        components.remove(ISA_TYPE);
        components.remove(PREFAB_ENTITY_ID_TYPE);
        let parent = resolved_node
            .parent
            .and_then(|p| old_to_new.get(&p).copied());
        prefab_entries.push(JsnEntity { parent, components });
    }

    // Assign fresh sequential PrefabEntityIds 1..N.
    for (i, entry) in prefab_entries.iter_mut().enumerate() {
        entry.components.insert(
            PREFAB_ENTITY_ID_TYPE.to_string(),
            serde_json::json!((i + 1) as u32),
        );
    }

    // Synthetic root takes its Name from the target file stem; its
    // Transform is identity in the prefab file (the instance's
    // placement transform stays in the scene file).
    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PrefabRoot")
        .to_string();
    let synthetic_entry = JsnEntity {
        parent: None,
        components: synthetic_root_components(display_name),
    };

    let mut final_entities: Vec<JsnEntity> = Vec::with_capacity(prefab_entries.len() + 1);
    final_entities.push(synthetic_entry);
    final_entities.extend(prefab_entries);

    let prefab_jsn = JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        assets: JsnAssets::default(),
        editor: None,
        scene: final_entities,
    };
    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = match serde_json::to_string_pretty(&prefab_jsn) {
        Ok(t) => t,
        Err(err) => {
            warn!("propagate_instance_to_prefab: serialize failed: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(target_path, &text) {
        warn!(
            "propagate_instance_to_prefab: failed to write {}: {err}",
            target_path.display()
        );
        return;
    }

    let prefab_ast = SceneJsnAst::from_jsn_scene(&prefab_jsn, &[]);
    world
        .resource_mut::<PrefabAstCache>()
        .insert(target_path, prefab_ast);
    if let Ok(fp) = crate::prefab::cache::compute_file_fingerprint(target_path) {
        world
            .resource_mut::<PrefabAstCache>()
            .record_saved_fingerprint(target_path, fp);
    }

    // Clear local override / local-only entries under the instance.
    // After propagation those values live in the prefab; respawning
    // resolves them as inherited.
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();
        let child_keys: Vec<usize> = ast.children_of(instance_key).collect();
        let mut entities_to_remove: Vec<Entity> = Vec::new();
        for child_key in child_keys {
            let descendants: Vec<usize> = ast.descendants_of(child_key);
            for d in std::iter::once(child_key).chain(descendants) {
                if let Some(node) = ast.nodes.get(d)
                    && let Some(e) = node.ecs_entity
                {
                    entities_to_remove.push(e);
                }
            }
        }
        for entity in entities_to_remove {
            ast.remove_node(entity);
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Shift the `translation` field of a Transform JSON value by `offset`.
/// Accepts either the array form (`[x, y, z]`) or the struct form
/// (`{ "x": .., "y": .., "z": .. }`). Returns the value untouched if no
/// recognised translation shape is present.
fn shift_translation_value(
    mut value: serde_json::Value,
    offset: bevy::math::Vec3,
) -> serde_json::Value {
    let serde_json::Value::Object(ref mut map) = value else {
        return value;
    };
    let Some(translation) = map.get_mut("translation") else {
        return value;
    };
    match translation {
        serde_json::Value::Array(arr) if arr.len() >= 3 => {
            let x = arr[0].as_f64().unwrap_or(0.0) as f32 + offset.x;
            let y = arr[1].as_f64().unwrap_or(0.0) as f32 + offset.y;
            let z = arr[2].as_f64().unwrap_or(0.0) as f32 + offset.z;
            arr[0] = serde_json::json!(x);
            arr[1] = serde_json::json!(y);
            arr[2] = serde_json::json!(z);
        }
        serde_json::Value::Object(t_map) => {
            for (axis, delta) in [("x", offset.x), ("y", offset.y), ("z", offset.z)] {
                let current = t_map
                    .get(axis)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0) as f32;
                t_map.insert(axis.to_string(), serde_json::json!(current + delta));
            }
        }
        _ => {}
    }
    value
}

/// Convenience wrapper that mutates a Transform entry inside a
/// `JsnEntity`'s component map in place. No-op if the entity has no
/// Transform component yet.
fn shift_transform_translation(
    components: &mut HashMap<String, serde_json::Value>,
    offset: bevy::math::Vec3,
) {
    let key = "bevy_transform::components::transform::Transform";
    let Some(value) = components.remove(key) else {
        return;
    };
    components.insert(key.to_string(), shift_translation_value(value, offset));
}

/// Remove a prefab instance wrapper, promoting its children to the
/// wrapper's former parent slot. Inherited children (materialised by
/// the resolver, ECS-only) are first promoted to authored AST nodes
/// carrying their full component set from the prefab cache.
/// Local-only children of the instance (AST nodes with no
/// `PrefabEntityId`) are reparented unchanged.
pub fn unbundle_instance(world: &mut World, instance_root_key: usize) {
    // Validate target + capture the parent slot and prefab source path.
    let (instance_parent, prefab_path, instance_world_transform) = {
        let ast = world.resource::<SceneJsnAst>();
        let Some(isa_value) = ast.get_component_at(instance_root_key, ISA_TYPE) else {
            warn!("unbundle_instance: target is not an IsA instance");
            return;
        };
        let parent = ast.nodes.get(instance_root_key).and_then(|n| n.parent);
        let source = isa_value
            .get("source")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let placement = ast
            .get_component_at(
                instance_root_key,
                "bevy_transform::components::transform::Transform",
            )
            .cloned();
        (parent, source, placement)
    };

    // Resolve the source AST so we can read the full component set of
    // every inherited descendant. The resolver merges prefab values +
    // any local overrides; the merged values are what we promote to
    // authored entries.
    let resolved = {
        let unresolved = world.resource::<SceneJsnAst>().clone();
        let cache = world.resource::<PrefabAstCache>();
        match crate::prefab::resolver::resolve_scene(&unresolved, cache) {
            Ok(r) => r,
            Err(err) => {
                warn!("unbundle_instance: resolver failed: {err}");
                return;
            }
        }
    };

    // Snapshot the instance's promoted children before we mutate the
    // source AST. Each entry is (new_node_components, original_parent_idx_in_resolved).
    let resolved_descendants: Vec<usize> = resolved.descendants_of(instance_root_key);

    // For each inherited descendant, build a JsnEntityNode that
    // captures its resolved components (without prefab markers) and
    // its parent in the resolved AST. Top-of-subtree descendants get
    // reparented to `instance_parent`; deeper descendants stay
    // parented to their resolved-AST parent (which we'll rewire).
    let mut promoted: Vec<(usize, HashMap<String, serde_json::Value>, Option<usize>)> = Vec::new();
    for &d in &resolved_descendants {
        let Some(node) = resolved.nodes.get(d) else {
            continue;
        };
        let mut components = node.components.clone();
        components.remove(PREFAB_TYPE);
        components.remove(PREFAB_ENTITY_ID_TYPE);
        components.remove(ISA_TYPE);
        let parent_in_resolved = node.parent;
        promoted.push((d, components, parent_in_resolved));
    }

    // Apply the instance's placement Transform to top-of-subtree
    // promoted entries so unbundling preserves world positions. Top
    // entries are those whose resolved parent IS the instance root.
    let instance_offset: Option<bevy::math::Vec3> =
        instance_world_transform.as_ref().and_then(|t| {
            let arr = t.get("translation")?.as_array()?;
            Some(bevy::math::Vec3::new(
                arr.first()?.as_f64()? as f32,
                arr.get(1)?.as_f64()? as f32,
                arr.get(2)?.as_f64()? as f32,
            ))
        });

    let _ = prefab_path; // path captured but not currently load-bearing; kept for future hooks

    // Now mutate the source AST: insert authored nodes for each
    // promoted descendant, reparent local-only children, then remove
    // the instance node.
    {
        let mut ast = world.resource_mut::<SceneJsnAst>();

        // resolved_idx -> new authored node key in the source AST.
        let mut resolved_to_new: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::new();

        for (resolved_idx, mut components, parent_in_resolved) in promoted {
            let new_key = ast.add_root();
            for (type_path, value) in components.drain() {
                ast.insert_component(new_key, &type_path, value);
            }

            // Resolve the new parent for this node:
            // - If its resolved parent IS the instance, it's a top
            //   subtree node; reparent to `instance_parent` and apply
            //   the instance's placement transform offset.
            // - Otherwise its resolved parent should already be in
            //   `resolved_to_new` because we iterate in BFS order from
            //   the resolver's `descendants_of`.
            let new_parent = if parent_in_resolved == Some(instance_root_key) {
                if let Some(offset) = instance_offset {
                    let transform_path = "bevy_transform::components::transform::Transform";
                    let current = ast.get_component_at(new_key, transform_path).cloned();
                    let next = match current {
                        Some(v) => shift_translation_value(v, offset),
                        None => serde_json::json!({
                            "translation": [offset.x, offset.y, offset.z],
                            "rotation": [0.0, 0.0, 0.0, 1.0],
                            "scale": [1.0, 1.0, 1.0],
                        }),
                    };
                    ast.replace_component(new_key, transform_path, next);
                }
                instance_parent
            } else {
                parent_in_resolved.and_then(|p| resolved_to_new.get(&p).copied())
            };
            if let Some(node) = ast.nodes.get_mut(new_key) {
                node.parent = new_parent;
            }

            resolved_to_new.insert(resolved_idx, new_key);
        }

        // Reparent any pre-existing local-only AST children of the
        // instance (nodes that were authored under the instance in the
        // source scene with no PrefabEntityId).
        let local_children: Vec<usize> = ast.children_of(instance_root_key).collect();
        for child_key in local_children {
            if let Some(node) = ast.nodes.get_mut(child_key) {
                node.parent = instance_parent;
            }
        }

        // Finally, wipe the instance node. We don't `remove_node`
        // because that would shift every other index; clearing its
        // components and detaching it leaves the spawn path treating
        // it as inert.
        if let Some(node) = ast.nodes.get_mut(instance_root_key) {
            node.components.clear();
            node.derived_components.clear();
            node.parent = None;
            if let Some(e) = node.ecs_entity.take() {
                ast.ecs_to_jsn.remove(&e);
            }
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Convert the active scene tab into a prefab tab. Writes a prefab
/// file at `target_path` containing the live `SceneJsnAst`'s
/// contents (with `Prefab` + `PrefabEntityId(0)` markers added to
/// the root entity), then mutates the active tab so its content,
/// kind, path, and `display_name` reflect the new prefab.
///
/// If the active scene has multiple top-level entities, a synthetic
/// prefab root is inserted (same pattern as
/// `save_as_prefab_from_selection`) so the prefab file always has a
/// single root.
pub fn save_scene_as_prefab(world: &mut World, target_path: &Path) {
    let mut prefab_ast = world.resource::<SceneJsnAst>().clone();

    // Strip stale prefab markers from every node so the new prefab
    // starts clean.
    for node in prefab_ast.nodes.iter_mut() {
        node.components.remove(PREFAB_TYPE);
        node.components.remove(ISA_TYPE);
        node.components.remove(PREFAB_ENTITY_ID_TYPE);
    }

    let roots: Vec<usize> = prefab_ast
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| if n.parent.is_none() { Some(i) } else { None })
        .collect();
    if roots.is_empty() {
        warn!("save_scene_as_prefab: active scene has no roots; nothing to save");
        return;
    }

    if roots.len() == 1 {
        let root = roots[0];
        if let Some(node) = prefab_ast.nodes.get_mut(root) {
            node.components
                .insert(PREFAB_TYPE.to_string(), serde_json::Value::Null);
            node.components
                .insert(PREFAB_ENTITY_ID_TYPE.to_string(), serde_json::json!(0));
        }
        let descendants = prefab_ast.descendants_of(root);
        for (i, desc_idx) in descendants.iter().enumerate() {
            if let Some(node) = prefab_ast.nodes.get_mut(*desc_idx) {
                node.components.insert(
                    PREFAB_ENTITY_ID_TYPE.to_string(),
                    serde_json::json!((i + 1) as u32),
                );
            }
        }
    } else {
        // Synthetic root pattern: serialised prefabs (matching
        // `save_as_prefab_from_selection`) keep the synthetic root at
        // index 0, so prepend rather than append.
        use jackdaw_jsn::ast::JsnEntityNode;
        use std::collections::HashMap as StdHashMap;

        let descendants_per_root: Vec<(usize, Vec<usize>)> = roots
            .iter()
            .map(|&r| (r, prefab_ast.descendants_of(r)))
            .collect();

        let synthetic_name = target_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("prefab")
            .to_string();
        let mut synthetic_components: StdHashMap<String, serde_json::Value> = StdHashMap::new();
        synthetic_components.insert(PREFAB_TYPE.to_string(), serde_json::Value::Null);
        synthetic_components.insert(PREFAB_ENTITY_ID_TYPE.to_string(), serde_json::json!(0));
        synthetic_components.insert(
            "bevy_ecs::name::Name".to_string(),
            serde_json::Value::String(synthetic_name),
        );
        synthetic_components.insert(
            "bevy_transform::components::transform::Transform".to_string(),
            serde_json::json!({
                "translation": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
        synthetic_components.insert(
            "bevy_camera::visibility::Visibility".to_string(),
            serde_json::Value::String("Inherited".to_string()),
        );
        let synthetic_node = JsnEntityNode {
            parent: None,
            components: synthetic_components,
            derived_components: Default::default(),
            ecs_entity: None,
        };

        prefab_ast.nodes.insert(0, synthetic_node);
        // Every pre-existing index has shifted by +1.
        for node in prefab_ast.nodes.iter_mut().skip(1) {
            if let Some(p) = node.parent {
                node.parent = Some(p + 1);
            }
        }
        for v in prefab_ast.ecs_to_jsn.values_mut() {
            *v += 1;
        }
        let shifted_dirty: std::collections::HashSet<usize> =
            prefab_ast.dirty_indices.iter().map(|&i| i + 1).collect();
        prefab_ast.dirty_indices = shifted_dirty;

        let mut next_id: u32 = 1;
        for (root_idx, descendants) in descendants_per_root {
            let shifted_root = root_idx + 1;
            if let Some(node) = prefab_ast.nodes.get_mut(shifted_root) {
                node.parent = Some(0);
                node.components.insert(
                    PREFAB_ENTITY_ID_TYPE.to_string(),
                    serde_json::json!(next_id),
                );
                next_id += 1;
            }
            for d in descendants {
                let shifted = d + 1;
                if let Some(node) = prefab_ast.nodes.get_mut(shifted) {
                    node.components.insert(
                        PREFAB_ENTITY_ID_TYPE.to_string(),
                        serde_json::json!(next_id),
                    );
                    next_id += 1;
                }
            }
        }
    }

    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    {
        let mut cache = world.resource_mut::<PrefabAstCache>();
        cache.insert(target_path, prefab_ast.clone());
    }
    if let Err(err) = save_prefab_to_disk(world, target_path) {
        warn!("save_scene_as_prefab: write failed: {err}");
        return;
    }

    // Install the prefab AST into the live SceneJsnAst so the viewport
    // stays consistent. The live world holds the same entities but
    // they now carry the prefab markers.
    *world.resource_mut::<SceneJsnAst>() = prefab_ast;

    let canonical = crate::prefab::canonical_prefab_path(target_path);
    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("prefab")
        .to_string();
    if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
        let active = scenes.active;
        if let Some(tab) = scenes.tabs.get_mut(active) {
            tab.path = Some(target_path.to_path_buf());
            tab.kind = crate::scenes::TabKind::Prefab;
            tab.content = crate::scenes::TabContent::Prefab(canonical);
            tab.display_name = display_name;
            tab.dirty = false;
        }
    }

    if let Some(mut spath) = world.get_resource_mut::<crate::scene_io::SceneFilePath>() {
        spath.path = Some(target_path.to_string_lossy().into_owned());
    }
    let history_len = world
        .resource::<jackdaw_commands::CommandHistory>()
        .undo_stack
        .len();
    world
        .resource_mut::<crate::scene_io::SceneDirtyState>()
        .undo_len_at_save = history_len;
    if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
        let active = scenes.active;
        if let Some(tab) = scenes.tabs.get_mut(active) {
            tab.history_depth_at_last_check = history_len;
        }
    }
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
    allows_undo = true
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
    allows_undo = true,
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
    allows_undo = true,
    params(
        entity(Entity, doc = "ECS entity of the instance entity."),
        type_path(String, doc = "Fully-qualified component type path."),
        field_path(String, doc = "Dotted field path within the component."),
    )
)]
pub fn prefab_revert_field(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        warn!("prefab.revert_field: missing `entity` param");
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
        // Resolve the AST key INSIDE the operator. The framework's
        // before-snapshot capture rebuilds the live AST which reshuffles
        // node indices, so a pre-resolved key would be stale.
        let key = world.resource::<SceneJsnAst>().key_for_entity(entity);
        let Some(key) = key else {
            warn!("prefab.revert_field: entity {entity:?} is not in the live AST");
            return;
        };
        revert_field(world, key, &type_path, &field_path);
    });
    OperatorResult::Finished
}

/// Revert an entire component on a prefab-instance entity back to the
/// prefab's inherited value.
#[operator(
    id = "prefab.revert_component",
    label = "Revert Component to Prefab",
    description = "Restore the component on a prefab-instance entity to its inherited prefab value.",
    allows_undo = true,
    params(
        entity(Entity, doc = "ECS entity of the instance entity."),
        type_path(String, doc = "Fully-qualified component type path."),
    )
)]
pub fn prefab_revert_component(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        warn!("prefab.revert_component: missing `entity` param");
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        warn!("prefab.revert_component: missing `type_path` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        // Resolve the AST key INSIDE the operator (see prefab.revert_field
        // for the rationale).
        let key = world.resource::<SceneJsnAst>().key_for_entity(entity);
        let Some(key) = key else {
            warn!("prefab.revert_component: entity {entity:?} is not in the live AST");
            return;
        };
        revert_component(world, key, &type_path);
    });
    OperatorResult::Finished
}

/// Revert every override on a prefab-instance subtree.
#[operator(
    id = "prefab.revert_all",
    label = "Revert All Overrides",
    description = "Remove every per-instance override on a prefab-instance subtree.",
    allows_undo = true,
    params(instance_entity(Entity, doc = "ECS entity of the instance root."),)
)]
pub fn prefab_revert_all(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(instance_entity) = params.as_entity("instance_entity") else {
        warn!("prefab.revert_all: missing `instance_entity` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        // Resolve the AST key INSIDE the operator (see prefab.revert_field
        // for the rationale).
        let key = world
            .resource::<SceneJsnAst>()
            .key_for_entity(instance_entity);
        let Some(key) = key else {
            warn!("prefab.revert_all: entity {instance_entity:?} is not in the live AST");
            return;
        };
        revert_all(world, key);
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
    allows_undo = true,
    params(
        instance_entity(Entity, doc = "ECS entity of the prefab-instance root."),
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
    let Some(instance_entity) = params.as_entity("instance_entity") else {
        warn!("prefab.apply_to_source: missing `instance_entity` param");
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
        // Resolve the AST key INSIDE the operator (see prefab.revert_field
        // for the rationale).
        let instance_root = world
            .resource::<SceneJsnAst>()
            .key_for_entity(instance_entity);
        let Some(instance_root) = instance_root else {
            warn!("prefab.apply_to_source: entity {instance_entity:?} is not in the live AST");
            return;
        };
        apply_to_prefab_source(
            world,
            instance_root,
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
    allows_undo = true,
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
    allows_undo = true,
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
    allows_undo = true,
    params(
        child_entity(Entity, doc = "ECS entity of the inherited child."),
        drop_target_entity(Entity, doc = "ECS entity to re-parent under."),
    )
)]
pub fn prefab_unpack_child(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(child_entity) = params.as_entity("child_entity") else {
        warn!("prefab.unpack_child: missing `child_entity` param");
        return OperatorResult::Cancelled;
    };
    let Some(drop_target_entity) = params.as_entity("drop_target_entity") else {
        warn!("prefab.unpack_child: missing `drop_target_entity` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        // Resolve both AST keys INSIDE the operator (see prefab.revert_field
        // for the rationale).
        let (child_key, drop_target_key) = {
            let ast = world.resource::<SceneJsnAst>();
            (
                ast.key_for_entity(child_entity),
                ast.key_for_entity(drop_target_entity),
            )
        };
        let Some(child_key) = child_key else {
            warn!("prefab.unpack_child: entity {child_entity:?} is not in the live AST");
            return;
        };
        let Some(drop_target_key) = drop_target_key else {
            warn!("prefab.unpack_child: entity {drop_target_entity:?} is not in the live AST");
            return;
        };
        unpack_child(world, child_key, drop_target_key);
    });
    OperatorResult::Finished
}

/// Remove a prefab instance wrapper, promoting its inherited children
/// to the instance's parent slot.
#[operator(
    id = "prefab.unbundle_instance",
    label = "Unbundle Prefab Instance",
    description = "Remove the prefab instance wrapper, leaving its inherited children as standalone entities.",
    allows_undo = true,
    params(instance_entity(Entity, doc = "ECS entity of the instance to unbundle."),)
)]
pub fn prefab_unbundle_instance(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(instance_entity) = params.as_entity("instance_entity") else {
        warn!("prefab.unbundle_instance: missing `instance_entity` param");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        // Resolve the AST key INSIDE the operator. Dispatching with a
        // pre-resolved key is unsafe: the framework's before-snapshot
        // capture rebuilds the live AST (prefabify_inherited_descendants
        // reshuffles node indices), so a key fetched before the operator
        // started can point at a different node by the time we read it.
        // Looking it up here uses the post-install live AST.
        let key = world
            .resource::<SceneJsnAst>()
            .key_for_entity(instance_entity);
        let Some(key) = key else {
            warn!("prefab.unbundle_instance: entity {instance_entity:?} is not in the live AST");
            return;
        };
        unbundle_instance(world, key);
    });
    OperatorResult::Finished
}

/// Walk every cached prefab AST and strip `IsA` components whose
/// `source` resolves back to the prefab itself. Self-referencing `IsA`
/// entries are a poisoned state produced by older save paths that
/// re-saved an instance into the same file; the resolver fails on them
/// and the file behaves as a regular scene from then on.
#[operator(
    id = "prefab.repair_self_cycles",
    label = "Repair Self-Cycling Prefabs",
    description = "Walk every cached prefab and strip IsA components that reference the prefab itself.",
    allows_undo = true
)]
pub fn prefab_repair_self_cycles(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(repair_self_cycles_system);
    OperatorResult::Finished
}

pub fn repair_self_cycles_system(world: &mut World) {
    let paths: Vec<PathBuf> = {
        let cache = world.resource::<PrefabAstCache>();
        cache.paths().map(Path::to_path_buf).collect()
    };
    for path in paths {
        let canonical_target = crate::prefab::canonical_prefab_path(&path);
        let mut to_strip: Vec<usize> = Vec::new();
        {
            let cache = world.resource::<PrefabAstCache>();
            let Some(ast) = cache.get(&path) else {
                continue;
            };
            for (idx, node) in ast.nodes.iter().enumerate() {
                let Some(isa) = node.components.get(ISA_TYPE) else {
                    continue;
                };
                let Some(source) = isa.get("source").and_then(|v| v.as_str()) else {
                    continue;
                };
                let isa_target = crate::prefab::canonical_prefab_path(source);
                if isa_target == canonical_target {
                    to_strip.push(idx);
                }
            }
        }
        if to_strip.is_empty() {
            continue;
        }
        let mut cache = world.resource_mut::<PrefabAstCache>();
        cache.mutate(&path, |ast| {
            for idx in &to_strip {
                if let Some(node) = ast.nodes.get_mut(*idx) {
                    node.components.remove(ISA_TYPE);
                }
            }
        });
        if let Err(err) = save_prefab_to_disk(world, &path) {
            warn!(
                "prefab.repair_self_cycles: failed to write {}: {err}",
                path.display()
            );
        } else {
            info!(
                "prefab.repair_self_cycles: stripped self-IsA from {}",
                path.display()
            );
        }
    }
}
