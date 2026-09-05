//! User-facing prefab operators. Each function mutates world state
//! directly; UI hookups route to these via the operator system.

use crate::prefab::cache::PrefabAstCache;
use crate::prefab::resolver_bsn::{
    read_isa_deleted, read_isa_source, read_prefab_entity_id, set_whole_component, value_to_patch,
};
use bevy::ecs::hierarchy::{ChildOf, Children};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_bsn::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue, SceneBsnAst,
    emit_scene, get_bsn_field, set_bsn_field,
};
use std::path::{Path, PathBuf};

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";
const VISIBILITY_INHERITED_TYPE: &str = "bevy_camera::visibility::Visibility::Inherited";

fn require_str(params: &OperatorParameters, key: &str, op_id: &str) -> Option<String> {
    let value = params.as_str(key).map(str::to_string);
    if value.is_none() {
        warn!("{op_id}: missing `{key}` param");
    }
    value
}

fn require_entity(params: &OperatorParameters, key: &str, op_id: &str) -> Option<Entity> {
    let value = params.as_entity(key);
    if value.is_none() {
        warn!("{op_id}: missing `{key}` param");
    }
    value
}

fn require_int(params: &OperatorParameters, key: &str, op_id: &str) -> Option<i64> {
    let value = params.as_int(key);
    if value.is_none() {
        warn!("{op_id}: missing `{key}` param");
    }
    value
}

fn require_float(params: &OperatorParameters, key: &str, op_id: &str) -> Option<f64> {
    let value = params.as_float(key);
    if value.is_none() {
        warn!("{op_id}: missing `{key}` param");
    }
    value
}

/// Resolve an ECS entity to its live BSN document node. The live document is
/// rebuilt during the framework's before-snapshot capture, so callers resolve
/// this inside the queued closure rather than passing a pre-resolved node.
fn resolve_ast_key(world: &World, entity: Entity, op_id: &str) -> Option<Entity> {
    let node = world.resource::<SceneBsnAst>().ast_for(entity);
    if node.is_none() {
        warn!("{op_id}: entity {entity:?} is not in the live document");
    }
    node
}

/// The `Prefab` unit-marker patch.
fn prefab_marker_patch() -> BsnPatch {
    BsnPatch::Type(PREFAB_TYPE.to_string())
}

/// A `PrefabEntityId(id)` tuple-struct patch.
fn peid_patch(id: u32) -> BsnPatch {
    value_to_patch(peid_value(id)).expect("PrefabEntityId is a tuple struct")
}

/// The `PrefabEntityId(id)` whole-component value.
fn peid_value(id: u32) -> BsnValue {
    BsnValue::TupleStruct(BsnTupleStructData {
        type_path: PREFAB_ENTITY_ID_TYPE.to_string(),
        values: vec![BsnValue::Int(i128::from(id))],
    })
}

fn field(name: &str, value: BsnValue) -> BsnField {
    BsnField {
        name: name.to_string(),
        value,
    }
}

/// An `IsA { source, deleted }` struct patch. Paths serialize as a plain
/// string; the deleted list is a list of integer ids.
fn isa_patch(source: &str, deleted: &[u32]) -> BsnPatch {
    value_to_patch(isa_value(source, deleted)).expect("IsA is a struct")
}

fn vec3_value(x: f32, y: f32, z: f32) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: "glam::Vec3".to_string(),
        fields: BsnStructFields(vec![
            field("x", BsnValue::Float(f64::from(x))),
            field("y", BsnValue::Float(f64::from(y))),
            field("z", BsnValue::Float(f64::from(z))),
        ]),
    })
}

fn quat_value(x: f32, y: f32, z: f32, w: f32) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: "glam::Quat".to_string(),
        fields: BsnStructFields(vec![
            field("x", BsnValue::Float(f64::from(x))),
            field("y", BsnValue::Float(f64::from(y))),
            field("z", BsnValue::Float(f64::from(z))),
            field("w", BsnValue::Float(f64::from(w))),
        ]),
    })
}

/// A `Transform` struct patch carrying only `translation` (a sparse delta the
/// resolver merges onto the inherited baseline).
fn transform_translation_patch(pos: Vec3) -> BsnPatch {
    BsnPatch::Struct(BsnStructData {
        type_path: TRANSFORM_TYPE.to_string(),
        fields: BsnStructFields(vec![field("translation", vec3_value(pos.x, pos.y, pos.z))]),
    })
}

/// The type path a patch stores a value for, if any.
fn patch_type_path(patch: &BsnPatch) -> Option<&str> {
    match patch {
        BsnPatch::Type(tp) | BsnPatch::Template(tp, _) => Some(tp),
        BsnPatch::Struct(data) => Some(&data.type_path),
        BsnPatch::TupleStruct(data) => Some(&data.type_path),
        _ => None,
    }
}

/// Deep-clone a live-document node's component patches into a fresh vector.
/// `Children` patches are dropped (the caller rebuilds the hierarchy); when
/// `drop_markers` is set, the `Prefab`, `IsA`, and `PrefabEntityId` markers are
/// dropped too. `Name` and every other component patch pass through so a prefab
/// file keeps them.
fn copy_component_patches(live: &SceneBsnAst, node: Entity, drop_markers: bool) -> Vec<BsnPatch> {
    let mut patches = live.cloned_component_patches(node);
    if drop_markers {
        patches.retain(|patch| {
            patch_type_path(patch)
                .map(|tp| tp != PREFAB_TYPE && tp != ISA_TYPE && tp != PREFAB_ENTITY_ID_TYPE)
                .unwrap_or(true)
        });
    }
    patches
}

/// Components for the synthetic prefab root: `Prefab` marker, `PrefabEntityId(0)`,
/// `Name = display_name`, identity `Transform`, and `Visibility::Inherited`.
/// Visibility is load-bearing for hierarchy propagation, so the prefab carries
/// it for the resolver to merge onto each instance.
fn synthetic_root_patches(display_name: &str) -> Vec<BsnPatch> {
    vec![
        prefab_marker_patch(),
        peid_patch(0),
        BsnPatch::Name(display_name.to_string()),
        BsnPatch::Struct(BsnStructData {
            type_path: TRANSFORM_TYPE.to_string(),
            fields: BsnStructFields(vec![
                field("translation", vec3_value(0.0, 0.0, 0.0)),
                field("rotation", quat_value(0.0, 0.0, 0.0, 1.0)),
                field("scale", vec3_value(1.0, 1.0, 1.0)),
            ]),
        }),
        BsnPatch::Type(VISIBILITY_INHERITED_TYPE.to_string()),
    ]
}

/// Make a cached source document instanceable as a prefab. The resolver
/// only materializes an `IsA` reference whose target has a single root
/// carrying the `Prefab` marker, so a plain scene (no marker, possibly
/// several roots) is wrapped into that shape here: a lone root is marked
/// in place, multiple roots are reparented under a synthetic `Prefab` root,
/// and every entity gets a sequential `PrefabEntityId`. A file that is
/// already a prefab passes through untouched, so real prefab sources are
/// unaffected. `display_name` names the synthetic root when one is made.
pub(crate) fn normalize_as_prefab_source(ast: &mut SceneBsnAst, display_name: &str) {
    let roots = ast.roots.clone();
    if roots.is_empty()
        || roots
            .iter()
            .any(|&root| ast.find_patch_by_type_path(root, PREFAB_TYPE).is_some())
    {
        return;
    }

    if roots.len() == 1 {
        let root = roots[0];
        set_whole_component(
            ast,
            root,
            PREFAB_TYPE,
            BsnValue::Type(PREFAB_TYPE.to_string()),
        );
        set_whole_component(ast, root, PREFAB_ENTITY_ID_TYPE, peid_value(0));
        for (i, desc) in ast.descendants_of(root).into_iter().enumerate() {
            set_whole_component(ast, desc, PREFAB_ENTITY_ID_TYPE, peid_value((i + 1) as u32));
        }
    } else {
        let synth = ast.create_entity_node(synthetic_root_patches(display_name));
        let mut next_id: u32 = 1;
        for root in roots {
            ast.move_to_parent(root, None, Some(synth));
            set_whole_component(ast, root, PREFAB_ENTITY_ID_TYPE, peid_value(next_id));
            next_id += 1;
            for desc in ast.descendants_of(root) {
                set_whole_component(ast, desc, PREFAB_ENTITY_ID_TYPE, peid_value(next_id));
                next_id += 1;
            }
        }
        ast.add_to_roots(synth);
    }
}

/// Shift the `translation` field of a Transform `BsnValue` by `offset`. No-op
/// when the value is not a Transform struct with a Vec3 translation.
fn shift_bsn_translation(value: &mut BsnValue, offset: Vec3) {
    let BsnValue::Struct(data) = value else {
        return;
    };
    let Some(translation) = data.fields.0.iter_mut().find(|f| f.name == "translation") else {
        return;
    };
    let BsnValue::Struct(vec3) = &mut translation.value else {
        return;
    };
    for (axis, delta) in [("x", offset.x), ("y", offset.y), ("z", offset.z)] {
        if let Some(comp) = vec3.fields.0.iter_mut().find(|f| f.name == axis)
            && let BsnValue::Float(current) = &mut comp.value
        {
            *current += f64::from(delta);
        }
    }
}

/// Shift the live node's Transform translation by `offset`, creating a
/// translation-only Transform when the node lacks one.
fn shift_node_translation(live: &mut SceneBsnAst, node: Entity, offset: Vec3) {
    match get_bsn_field(live, node, TRANSFORM_TYPE, "") {
        Some(mut value) => {
            shift_bsn_translation(&mut value, offset);
            set_whole_component(live, node, TRANSFORM_TYPE, value);
        }
        None => {
            let value = BsnValue::Struct(BsnStructData {
                type_path: TRANSFORM_TYPE.to_string(),
                fields: BsnStructFields(vec![field(
                    "translation",
                    vec3_value(offset.x, offset.y, offset.z),
                )]),
            });
            set_whole_component(live, node, TRANSFORM_TYPE, value);
        }
    }
}

/// Map a prefab target path to the `.bsn` file that actually gets written.
/// Prefabs persist as BSN text (the cache loader reads only `.bsn`), so a
/// legacy `.jsn` target redirects to its `.bsn` sibling. `IsA` sources still
/// pointing at the old `.jsn` resolve through `resolve_source_path`'s sibling
/// fallback.
fn prefab_bsn_path(target_path: &Path) -> PathBuf {
    if target_path.extension().is_some_and(|e| e == "jsn") {
        target_path.with_extension("bsn")
    } else {
        target_path.to_path_buf()
    }
}

/// Rename an existing legacy `.jsn` prefab to `.jsn.bak` (the same convention
/// scene saves use) so a stale `.jsn` cannot shadow the fresh `.bsn` sibling.
fn back_up_legacy_prefab(original: &Path) {
    if !original.exists() {
        return;
    }
    let mut backup = original.as_os_str().to_owned();
    backup.push(".bak");
    if let Err(err) = std::fs::rename(original, &backup) {
        warn!(
            "could not back up legacy prefab {}: {err}",
            original.display()
        );
    }
}

/// Write a prefab document as BSN text. A `.jsn` target redirects to its
/// `.bsn` sibling, backing up the legacy file. Returns the path actually
/// written, or `None` on write failure.
fn write_prefab_doc(target_path: &Path, prefab: &SceneBsnAst, op_id: &str) -> Option<PathBuf> {
    let path = prefab_bsn_path(target_path);
    if path != target_path {
        back_up_legacy_prefab(target_path);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(err) = std::fs::write(&path, emit_scene(prefab)) {
        warn!("{op_id}: failed to write {}: {err}", path.display());
        return None;
    }
    Some(path)
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
        // Skip ECS-only derived children (brush clip overlays, face
        // entities, etc.) that have no document node. Persisting them as
        // top-level prefab entries would orphan them after respawn. The
        // brush spawn pipeline recreates them from the brush data.
        if world.resource::<SceneBsnAst>().ast_for(child).is_none() {
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

/// Centroid of the given roots, preferring `GlobalTransform` (so a selection
/// under a non-identity parent bundles relative to its real world position)
/// and falling back to `Transform`.
fn selection_centroid(world: &World, roots: &[Entity]) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    for &root in roots {
        if let Some(gt) = world.get::<GlobalTransform>(root) {
            sum += gt.translation();
            count += 1;
        } else if let Some(t) = world.get::<Transform>(root) {
            sum += t.translation;
            count += 1;
        }
    }
    if count > 0 {
        sum / count as f32
    } else {
        Vec3::ZERO
    }
}

/// Save the given roots (and their descendants) as a prefab file, then
/// replace them in the source scene with a fresh instance.
///
/// Selection normalization runs first: any entity whose ancestor is also
/// in `roots` gets dropped (its parent already covers it). The remaining
/// "top roots" are the ones that get packaged.
///
/// If a selected root carries `IsA`, this is the **propagate** path:
/// the instance's current resolved state is written back to the prefab
/// file. The instance entity stays at its current scene position.
///
/// Otherwise this is the **bundle** path: the prefab file is written from
/// the selection, the selection is removed from the source scene, and a
/// fresh instance is spawned at the selection's centroid via
/// `spawn_instance`.
pub fn save_as_prefab_from_selection(world: &mut World, roots: &[Entity], target_path: &Path) {
    let normalized = normalize_selection_roots(world, roots);
    if normalized.is_empty() {
        warn!("save_as_prefab_from_selection: empty selection");
        return;
    }

    // Propagate only when the selection is a single instance root whose IsA
    // source matches the target and whose prefab is cached. Anything else
    // falls through to the bundle path.
    let propagate_target = if normalized.len() == 1 {
        let root = normalized[0];
        let live = world.resource::<SceneBsnAst>();
        let cache = world.resource::<PrefabAstCache>();
        live.ast_for(root).and_then(|node| {
            let source = read_isa_source(live, node)?;
            // A legacy `.jsn` source or target still names the same prefab
            // as its `.bsn` sibling, so compare the redirected forms.
            if prefab_bsn_path(&source) == prefab_bsn_path(target_path)
                && cache.get(&prefab_bsn_path(target_path)).is_some()
            {
                Some(node)
            } else {
                None
            }
        })
    } else {
        None
    };
    if let Some(instance_node) = propagate_target {
        propagate_instance_to_prefab(world, instance_node, target_path);
        return;
    }

    save_selection_as_new_prefab(world, &normalized, target_path);
}

/// Bundle path: write a fresh prefab file from the selection and replace it
/// in the source scene with an instance of the new prefab.
fn save_selection_as_new_prefab(world: &mut World, normalized: &[Entity], target_path: &Path) {
    // BFS each top root in input order so `PrefabEntityId` assignment 1..N is
    // stable across runs.
    let mut entities: Vec<Entity> = Vec::new();
    for &root in normalized {
        entities.push(root);
        collect_descendants(world, root, &mut entities);
    }

    let top_root_set: std::collections::HashSet<Entity> = normalized.iter().copied().collect();
    let centroid = selection_centroid(world, normalized);
    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PrefabRoot")
        .to_string();

    // Build the prefab document from the live document's subtree.
    let mut prefab = SceneBsnAst::default();
    let synth = prefab.create_entity_node(synthetic_root_patches(&display_name));
    prefab.add_to_roots(synth);

    let mut ecs_to_prefab: std::collections::HashMap<Entity, Entity> =
        std::collections::HashMap::new();
    {
        let live = world.resource::<SceneBsnAst>();
        for (i, &entity) in entities.iter().enumerate() {
            let Some(node) = live.ast_for(entity) else {
                continue;
            };
            let mut patches = copy_component_patches(live, node, true);
            patches.push(peid_patch((i + 1) as u32));
            let prefab_node = prefab.create_entity_node(patches);
            ecs_to_prefab.insert(entity, prefab_node);
        }
    }

    // Parent each packaged entity. Entities whose parent is also packaged nest
    // under it; top roots (parent not in the set) parent under the synthetic
    // root and have their translation shifted into its local frame.
    for &entity in &entities {
        let Some(&prefab_node) = ecs_to_prefab.get(&entity) else {
            continue;
        };
        let parent_ecs = world.get::<ChildOf>(entity).map(ChildOf::parent);
        let prefab_parent = match parent_ecs {
            Some(parent) if ecs_to_prefab.contains_key(&parent) => ecs_to_prefab[&parent],
            _ => {
                if top_root_set.contains(&entity)
                    && let Some(value) = get_bsn_field(&prefab, prefab_node, TRANSFORM_TYPE, "")
                {
                    let mut value = value;
                    shift_bsn_translation(&mut value, -centroid);
                    set_whole_component(&mut prefab, prefab_node, TRANSFORM_TYPE, value);
                }
                synth
            }
        };
        prefab.add_child_to_ast(prefab_parent, prefab_node);
    }

    let Some(target_path) = write_prefab_doc(target_path, &prefab, "save_as_prefab_from_selection")
    else {
        return;
    };

    // Remove the packaged entities from the live document so the upcoming
    // reload doesn't respawn them alongside the new instance.
    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        for &entity in &entities {
            live.remove_entity_node(entity);
        }
    }

    // spawn_instance adds the instance node and triggers a reload that
    // materializes the inherited children from the prefab we just wrote.
    spawn_instance(world, &target_path, centroid);
}

/// Propagate path: snapshot the instance's current subtree from the live
/// document and write it back to the prefab file at `target_path`. The
/// instance entity stays put; other instances pick up the new baseline on the
/// next cache-driven respawn.
fn propagate_instance_to_prefab(world: &mut World, instance_node: Entity, target_path: &Path) {
    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("PrefabRoot")
        .to_string();

    let mut prefab = SceneBsnAst::default();
    let synth = prefab.create_entity_node(synthetic_root_patches(&display_name));
    prefab.add_to_roots(synth);

    // Map the instance root to the synthetic prefab root; every live descendant
    // becomes a prefab child entry with a fresh sequential PrefabEntityId.
    let descendants: Vec<Entity> = {
        let live = world.resource::<SceneBsnAst>();
        live.descendants_of(instance_node)
    };
    let mut map: std::collections::HashMap<Entity, Entity> = std::collections::HashMap::new();
    map.insert(instance_node, synth);
    {
        let live = world.resource::<SceneBsnAst>();
        for (i, &d) in descendants.iter().enumerate() {
            let mut patches = copy_component_patches(live, d, true);
            patches.push(peid_patch((i + 1) as u32));
            let prefab_node = prefab.create_entity_node(patches);
            map.insert(d, prefab_node);
        }
        for &d in &descendants {
            let Some(&prefab_node) = map.get(&d) else {
                continue;
            };
            let live_parent = live.ast_parent_of(d).unwrap_or(instance_node);
            let prefab_parent = map.get(&live_parent).copied().unwrap_or(synth);
            prefab.add_child_to_ast(prefab_parent, prefab_node);
        }
    }

    let Some(target_path) = write_prefab_doc(target_path, &prefab, "propagate_instance_to_prefab")
    else {
        return;
    };

    world
        .resource_mut::<PrefabAstCache>()
        .insert(&target_path, prefab);
    if let Ok(fp) = crate::prefab::cache::compute_file_fingerprint(&target_path) {
        world
            .resource_mut::<PrefabAstCache>()
            .record_saved_fingerprint(&target_path, fp);
    }

    // Clear local override / local-only entries under the instance. After
    // propagation those values live in the prefab; respawning resolves them as
    // inherited.
    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let child_nodes = live.get_children_ast(instance_node);
        let mut entities_to_remove: Vec<Entity> = Vec::new();
        for child in child_nodes {
            let subtree = std::iter::once(child).chain(live.descendants_of(child));
            for node in subtree {
                if let Some(e) = live.ecs_for_ast(node) {
                    entities_to_remove.push(e);
                }
            }
        }
        for entity in entities_to_remove {
            live.remove_entity_node(entity);
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Remove a prefab instance wrapper, promoting its children to the wrapper's
/// former parent slot. In the full live document the inherited descendants
/// already carry their merged component set, so promotion strips the prefab
/// markers and reparents; the instance's placement Transform is folded into the
/// top-of-subtree children so world positions are preserved.
pub fn unbundle_instance(world: &mut World, instance_root_node: Entity) {
    let (instance_parent, placement_offset) = {
        let live = world.resource::<SceneBsnAst>();
        if live
            .find_patch_by_type_path(instance_root_node, ISA_TYPE)
            .is_none()
        {
            warn!("unbundle_instance: target is not an IsA instance");
            return;
        }
        let parent = live.ast_parent_of(instance_root_node);
        let offset =
            get_bsn_field(live, instance_root_node, TRANSFORM_TYPE, "").and_then(|value| {
                let BsnValue::Struct(data) = value else {
                    return None;
                };
                let translation = data
                    .fields
                    .0
                    .into_iter()
                    .find(|f| f.name == "translation")?;
                let BsnValue::Struct(vec3) = translation.value else {
                    return None;
                };
                let axis = |name: &str| {
                    vec3.fields.0.iter().find(|f| f.name == name).and_then(|f| {
                        if let BsnValue::Float(v) = f.value {
                            Some(v as f32)
                        } else {
                            None
                        }
                    })
                };
                Some(Vec3::new(axis("x")?, axis("y")?, axis("z")?))
            });
        (parent, offset)
    };

    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let children = live.get_children_ast(instance_root_node);

        // Strip prefab markers across the whole promoted subtree so the
        // detached entities read as standalone.
        let mut subtree: Vec<Entity> = Vec::new();
        for &child in &children {
            subtree.push(child);
            subtree.extend(live.descendants_of(child));
        }
        for node in subtree {
            live.remove_component_patch(node, PREFAB_TYPE);
            live.remove_component_patch(node, PREFAB_ENTITY_ID_TYPE);
            live.remove_component_patch(node, ISA_TYPE);
        }

        // Fold the instance placement into each top-of-subtree child, then
        // reparent them to the instance's former parent.
        for &child in &children {
            if let Some(offset) = placement_offset {
                shift_node_translation(&mut live, child, offset);
            }
            live.move_to_parent(child, Some(instance_root_node), instance_parent);
        }

        // Remove the now-empty instance node.
        if let Some(entity) = live.ecs_for_ast(instance_root_node) {
            live.remove_entity_node(entity);
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Convert the active scene tab into a prefab. Writes a prefab file at
/// `target_path` from the live document (with `Prefab` + `PrefabEntityId`
/// markers added), mutates the live document to carry the markers, and updates
/// the active tab so its content, kind, path, and `display_name` reflect the
/// new prefab.
pub fn save_scene_as_prefab(world: &mut World, target_path: &Path) {
    // Prefabs persist as BSN text, so the cache entry, tab path, and file
    // path all use the `.bsn` form of the target.
    let bsn_target = prefab_bsn_path(target_path);
    if bsn_target != target_path {
        back_up_legacy_prefab(target_path);
    }
    let target_path = &bsn_target;
    let display_name = target_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("prefab")
        .to_string();

    let roots = world.resource::<SceneBsnAst>().roots.clone();
    if roots.is_empty() {
        warn!("save_scene_as_prefab: active scene has no roots; nothing to save");
        return;
    }

    {
        let mut live = world.resource_mut::<SceneBsnAst>();

        // Strip any stale markers from every node.
        let mut all_nodes: Vec<Entity> = Vec::new();
        for &root in &roots {
            all_nodes.push(root);
            all_nodes.extend(live.descendants_of(root));
        }
        for node in &all_nodes {
            live.remove_component_patch(*node, PREFAB_TYPE);
            live.remove_component_patch(*node, ISA_TYPE);
            live.remove_component_patch(*node, PREFAB_ENTITY_ID_TYPE);
        }

        if roots.len() == 1 {
            let root = roots[0];
            set_whole_component(
                &mut live,
                root,
                PREFAB_TYPE,
                BsnValue::Type(PREFAB_TYPE.to_string()),
            );
            set_whole_component(&mut live, root, PREFAB_ENTITY_ID_TYPE, peid_value(0));
            let descendants = live.descendants_of(root);
            for (i, desc) in descendants.iter().enumerate() {
                set_whole_component(
                    &mut live,
                    *desc,
                    PREFAB_ENTITY_ID_TYPE,
                    peid_value((i + 1) as u32),
                );
            }
        } else {
            // Multiple top-level roots: introduce a synthetic prefab root and
            // reparent the existing roots under it with sequential ids.
            let synth = live.create_entity_node(synthetic_root_patches(&display_name));
            let mut next_id: u32 = 1;
            for &root in &roots {
                live.move_to_parent(root, None, Some(synth));
                set_whole_component(&mut live, root, PREFAB_ENTITY_ID_TYPE, peid_value(next_id));
                next_id += 1;
                for desc in live.descendants_of(root) {
                    set_whole_component(
                        &mut live,
                        desc,
                        PREFAB_ENTITY_ID_TYPE,
                        peid_value(next_id),
                    );
                    next_id += 1;
                }
            }
            live.add_to_roots(synth);
        }
    }

    // Cache and persist the prefab document.
    {
        let prefab = crate::prefab::resolver_bsn::clone_scene(world.resource::<SceneBsnAst>());
        world
            .resource_mut::<PrefabAstCache>()
            .insert(target_path, prefab);
    }
    if let Err(err) = save_prefab_to_disk(world, target_path) {
        warn!("save_scene_as_prefab: write failed: {err}");
        return;
    }

    let canonical = crate::prefab::canonical_prefab_path(target_path);
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

/// Add a new prefab instance to the live scene at `world_pos`. Caches the
/// prefab document if missing, adds an instance root carrying
/// `IsA + PrefabEntityId + Transform` (a translation-only sparse delta), then
/// resolves + respawns the scene preview.
pub fn spawn_instance(world: &mut World, prefab_path: &Path, world_pos: Vec3) {
    let already_cached = world
        .resource::<PrefabAstCache>()
        .get(prefab_path)
        .is_some();
    if !already_cached {
        match crate::prefab::save_load::read_prefab_ast(prefab_path) {
            Ok(prefab) => {
                world
                    .resource_mut::<PrefabAstCache>()
                    .insert(prefab_path, prefab);
            }
            Err(e) => {
                warn!(
                    "spawn_instance: failed to read prefab {}: {e}",
                    prefab_path.display()
                );
                return;
            }
        }
    }

    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let source = prefab_path.to_string_lossy().into_owned();
        let node = live.create_entity_node(vec![
            isa_patch(&source, &[]),
            peid_patch(0),
            transform_translation_patch(world_pos),
        ]);
        live.add_to_roots(node);
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// The prefab baseline value (whole component when `field_path` is empty, else
/// the leaf at `field_path`) for the entity's `(source, PrefabEntityId)`.
fn resolve_prefab_value(
    world: &World,
    node: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<BsnValue> {
    let live = world.resource::<SceneBsnAst>();
    let cache = world.resource::<PrefabAstCache>();
    let peid = read_prefab_entity_id(live, node)?;
    let isa_node = live.ancestor_with_component(node, ISA_TYPE)?;
    let source = read_isa_source(live, isa_node)?;
    let prefab = cache.get(&source)?;
    let prefab_node = prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))?;
    get_bsn_field(prefab, prefab_node, type_path, field_path)
}

/// Revert one component field on a prefab-instance entity to its inherited
/// value. After mutating the document, respawns so the live preview reflects
/// the revert.
pub fn revert_field(world: &mut World, node: Entity, type_path: &str, field_path: &str) {
    let Some(prefab_leaf) = resolve_prefab_value(world, node, type_path, field_path) else {
        return;
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    {
        let reg = registry.read();
        let mut live = world.resource_mut::<SceneBsnAst>();
        if get_bsn_field(&live, node, type_path, "").is_none() {
            return;
        }
        set_bsn_field(&mut live, node, type_path, field_path, prefab_leaf, &reg);
    }
    crate::prefab::watcher::reload_all_instances(world);
}

/// Revert an entire component on a prefab-instance entity to the prefab's
/// value. Bails if there is no resolvable prefab inheritance for the component;
/// removing in that case would silently destroy authored data.
pub fn revert_component(world: &mut World, node: Entity, type_path: &str) {
    let Some(prefab_value) = resolve_prefab_value(world, node, type_path, "") else {
        warn!(
            "revert_component: no prefab inheritance for node={node:?} \
             type_path={type_path}; refusing to remove the component"
        );
        return;
    };
    set_whole_component(
        &mut world.resource_mut::<SceneBsnAst>(),
        node,
        type_path,
        prefab_value,
    );
    crate::prefab::watcher::reload_all_instances(world);
}

/// Revert every override on an instance subtree. Walks the root and its
/// descendants; for each that has a `PrefabEntityId`, resets all non-marker
/// components to the prefab baseline while preserving `IsA` + `PrefabEntityId`.
pub fn revert_all(world: &mut World, instance_root_node: Entity) {
    let source = {
        let live = world.resource::<SceneBsnAst>();
        let Some(source) = read_isa_source(live, instance_root_node) else {
            return;
        };
        source
    };

    let mut targets: Vec<Entity> = Vec::new();
    {
        let live = world.resource::<SceneBsnAst>();
        let mut nodes = vec![instance_root_node];
        nodes.extend(live.descendants_of(instance_root_node));
        for node in nodes {
            if read_prefab_entity_id(live, node).is_some() {
                targets.push(node);
            }
        }
    }

    // Read the prefab baselines while borrowing the cache immutably, then apply
    // them; never clone the cache.
    let baselines: Vec<(Entity, Vec<(String, BsnValue)>)> = {
        let live = world.resource::<SceneBsnAst>();
        let cache = world.resource::<PrefabAstCache>();
        let Some(prefab) = cache.get(&source) else {
            return;
        };
        targets
            .into_iter()
            .filter_map(|node| {
                let peid = read_prefab_entity_id(live, node)?;
                let prefab_node =
                    prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))?;
                let comps: Vec<(String, BsnValue)> = prefab
                    .component_type_paths(prefab_node)
                    .into_iter()
                    .filter(|tp| tp != PREFAB_TYPE)
                    .filter_map(|tp| get_bsn_field(prefab, prefab_node, &tp, "").map(|v| (tp, v)))
                    .collect();
                Some((node, comps))
            })
            .collect()
    };

    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        for (node, comps) in baselines {
            let preserved_isa = get_bsn_field(&live, node, ISA_TYPE, "");
            let preserved_id = get_bsn_field(&live, node, PREFAB_ENTITY_ID_TYPE, "");
            for tp in live.component_type_paths(node) {
                live.remove_component_patch(node, &tp);
            }
            for (tp, value) in comps {
                if tp == PREFAB_TYPE {
                    continue;
                }
                set_whole_component(&mut live, node, &tp, value);
            }
            if let Some(isa) = preserved_isa {
                set_whole_component(&mut live, node, ISA_TYPE, isa);
            }
            if let Some(id) = preserved_id {
                set_whole_component(&mut live, node, PREFAB_ENTITY_ID_TYPE, id);
            }
        }
    }

    crate::prefab::watcher::reload_all_instances(world);
}

/// Convert an existing prefab instance into a new variant prefab. The new file
/// has its own `Prefab` marker AND `IsA` referencing the original, so it
/// inherits from the original while carrying the instance's current overrides
/// as its own base. The source scene's instance is rewired to the variant.
pub fn save_as_variant(world: &mut World, instance_root: Entity, target_path: &Path) {
    let (source, deleted, root_patches, descendant_data) = {
        let live = world.resource::<SceneBsnAst>();
        let Some(instance_node) = live.ast_for(instance_root) else {
            warn!("save_as_variant: instance entity not in document");
            return;
        };
        if live
            .find_patch_by_type_path(instance_node, ISA_TYPE)
            .is_none()
        {
            warn!("save_as_variant: instance lacks IsA");
            return;
        }
        let source = read_isa_source(live, instance_node).unwrap_or_default();
        let deleted = read_isa_deleted(live, instance_node);
        let root_patches = copy_component_patches(live, instance_node, true);
        let descendant_data: Vec<(u32, Vec<BsnPatch>)> = live
            .descendants_of(instance_node)
            .into_iter()
            .filter_map(|child| {
                let id = read_prefab_entity_id(live, child)?;
                Some((id, copy_component_patches(live, child, true)))
            })
            .collect();
        (source, deleted, root_patches, descendant_data)
    };

    // Build the variant document.
    let mut variant = SceneBsnAst::default();
    let mut root_node_patches = vec![
        prefab_marker_patch(),
        peid_patch(0),
        isa_patch(&source.to_string_lossy(), &deleted),
    ];
    root_node_patches.extend(root_patches);
    let variant_root = variant.create_entity_node(root_node_patches);
    variant.add_to_roots(variant_root);
    for (id, patches) in descendant_data {
        let mut child_patches = vec![peid_patch(id)];
        child_patches.extend(patches);
        let child = variant.create_entity_node(child_patches);
        variant.add_child_to_ast(variant_root, child);
    }

    let Some(target_path) = write_prefab_doc(target_path, &variant, "save_as_variant") else {
        return;
    };

    world
        .resource_mut::<PrefabAstCache>()
        .insert(&target_path, variant);

    // Rewire the source instance to the variant and clear its now-redundant
    // descendant overrides (they live in the variant's base now).
    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let Some(instance_node) = live.ast_for(instance_root) else {
            return;
        };
        let new_source = target_path.to_string_lossy().into_owned();
        set_whole_component(
            &mut live,
            instance_node,
            ISA_TYPE,
            isa_value(&new_source, &[]),
        );
        for child in live.descendants_of(instance_node) {
            for tp in live.component_type_paths(child) {
                if tp == PREFAB_ENTITY_ID_TYPE {
                    continue;
                }
                live.remove_component_patch(child, &tp);
            }
        }
    }
}

/// The `IsA { source, deleted }` whole-component value.
fn isa_value(source: &str, deleted: &[u32]) -> BsnValue {
    BsnValue::Struct(BsnStructData {
        type_path: ISA_TYPE.to_string(),
        fields: BsnStructFields(vec![
            field("source", BsnValue::String(source.to_string())),
            field(
                "deleted",
                BsnValue::List(
                    deleted
                        .iter()
                        .map(|&d| BsnValue::Int(i128::from(d)))
                        .collect(),
                ),
            ),
        ]),
    })
}

/// Apply a single-field value to every prefab instance in the scene that points
/// at `source_path`. The field path can be dotted (e.g. `"scale.x"`).
pub fn bulk_apply_in_scene(
    world: &mut World,
    source_path: &Path,
    type_path: &str,
    field_path: &str,
    value: BsnValue,
) {
    let source = source_path.to_string_lossy().to_string();
    let matches: Vec<Entity> = {
        let live = world.resource::<SceneBsnAst>();
        live.entities_with_component(ISA_TYPE)
            .into_iter()
            .filter(|&node| {
                read_isa_source(live, node)
                    .map(|s| s.to_string_lossy() == source)
                    .unwrap_or(false)
            })
            .collect()
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    {
        let reg = registry.read();
        let mut live = world.resource_mut::<SceneBsnAst>();
        for node in matches {
            set_bsn_field(&mut live, node, type_path, field_path, value.clone(), &reg);
        }
    }
    crate::prefab::watcher::reload_all_instances(world);
}

/// Apply a single-field value into a prefab's source document so the override
/// becomes the new inherited base. Mutates the cache in place; the resolve-on-
/// change driver picks up the epoch bump and respawns the active scene next
/// frame. Also clears the matching delta from the source instance.
pub fn apply_to_prefab_source(
    world: &mut World,
    instance_root_node: Entity,
    entity_id: u32,
    type_path: &str,
    field_path: &str,
    value: BsnValue,
) {
    let source_path: PathBuf = {
        let live = world.resource::<SceneBsnAst>();
        let Some(source) = read_isa_source(live, instance_root_node) else {
            warn!("apply_to_prefab_source: instance lacks IsA source");
            return;
        };
        source
    };

    let registry = world.resource::<AppTypeRegistry>().clone();
    let applied = {
        let reg = registry.read();
        world
            .resource_mut::<PrefabAstCache>()
            .mutate(&source_path, |prefab: &mut SceneBsnAst| {
                let Some(target) =
                    prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(entity_id))
                else {
                    warn!("apply_to_prefab_source: PrefabEntityId({entity_id}) not in prefab");
                    return;
                };
                set_bsn_field(prefab, target, type_path, field_path, value, &reg);
            })
    };
    if !applied {
        warn!(
            "apply_to_prefab_source: prefab not cached: {}",
            source_path.display()
        );
        return;
    }

    // Clear the matching top-level delta on the source-scene side.
    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let mut candidates = live.descendants_of(instance_root_node);
        candidates.push(instance_root_node);
        let scene_node = candidates
            .into_iter()
            .find(|&n| read_prefab_entity_id(&live, n) == Some(entity_id));
        if let Some(scene_node) = scene_node
            && let Some(BsnValue::Struct(mut data)) =
                get_bsn_field(&live, scene_node, type_path, "")
        {
            data.fields.0.retain(|f| f.name != field_path);
            if data.fields.0.is_empty() {
                live.remove_component_patch(scene_node, type_path);
            } else {
                set_whole_component(&mut live, scene_node, type_path, BsnValue::Struct(data));
            }
        }
    }
}

/// Remove an inherited child from its prefab instance and re-parent it under
/// `drop_target_node` as a standalone (non-instance) entity. The parent
/// instance's `IsA.deleted` list is extended with the child's `PrefabEntityId`
/// so the resolver no longer re-materializes it.
pub fn unpack_child(world: &mut World, child_node: Entity, drop_target_node: Entity) {
    let mut live = world.resource_mut::<SceneBsnAst>();

    let Some(id) = read_prefab_entity_id(&live, child_node) else {
        warn!("unpack_child: child lacks PrefabEntityId");
        return;
    };

    let Some(instance_node) = live.ancestor_with_component(child_node, ISA_TYPE) else {
        warn!("unpack_child: child has no IsA ancestor");
        return;
    };

    // Extend IsA.deleted on the instance root.
    let source = read_isa_source(&live, instance_node).unwrap_or_default();
    let mut deleted = read_isa_deleted(&live, instance_node);
    if !deleted.contains(&id) {
        deleted.push(id);
    }
    set_whole_component(
        &mut live,
        instance_node,
        ISA_TYPE,
        isa_value(&source.to_string_lossy(), &deleted),
    );

    // Copy the child's non-marker components under the drop target as a
    // standalone child.
    let component_pairs: Vec<(String, BsnValue)> = live
        .component_type_paths(child_node)
        .into_iter()
        .filter(|tp| tp != PREFAB_ENTITY_ID_TYPE)
        .filter_map(|tp| get_bsn_field(&live, child_node, &tp, "").map(|v| (tp, v)))
        .collect();
    let new_child = live.create_entity_node(Vec::new());
    live.add_child_to_ast(drop_target_node, new_child);
    for (tp, value) in component_pairs {
        set_whole_component(&mut live, new_child, &tp, value);
    }
}

/// Walk every entity in the prefab-instance subtree rooted at
/// `instance_root_node`. For each that carries a `PrefabEntityId`, diff its
/// non-marker components against the cached prefab's matching entity and call
/// `apply_to_prefab_source` for every overridden leaf. At the end the prefab
/// source holds all edits and the instance has no remaining overrides.
pub fn apply_all_overrides_to_source(world: &mut World, instance_root_node: Entity) {
    let prefab_path: PathBuf = {
        let live = world.resource::<SceneBsnAst>();
        let Some(source) = read_isa_source(live, instance_root_node) else {
            return;
        };
        source
    };

    // (prefab_entity_id, type_path, field_path, value) work items.
    let mut work: Vec<(u32, String, String, BsnValue)> = Vec::new();
    {
        let live = world.resource::<SceneBsnAst>();
        let cache = world.resource::<PrefabAstCache>();
        let Some(prefab) = cache.get(&prefab_path) else {
            return;
        };

        let mut pairs: Vec<(Entity, u32)> = Vec::new();
        if let Some(id) = read_prefab_entity_id(live, instance_root_node) {
            pairs.push((instance_root_node, id));
        }
        for descendant in live.descendants_of(instance_root_node) {
            if let Some(id) = read_prefab_entity_id(live, descendant) {
                pairs.push((descendant, id));
            }
        }

        for (node, peid) in pairs {
            let Some(prefab_node) =
                prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))
            else {
                continue;
            };
            for tp in live.component_type_paths(node) {
                if tp == PREFAB_TYPE || tp == ISA_TYPE || tp == PREFAB_ENTITY_ID_TYPE {
                    continue;
                }
                let Some(scene_value) = get_bsn_field(live, node, &tp, "") else {
                    continue;
                };
                let prefab_value = get_bsn_field(prefab, prefab_node, &tp, "");
                for (field_path, value) in crate::prefab::overrides_bsn::collect_overridden_paths(
                    &scene_value,
                    prefab_value.as_ref(),
                ) {
                    work.push((peid, tp.clone(), field_path, value));
                }
            }
        }
    }

    for (peid, type_path, field_path, value) in work {
        apply_to_prefab_source(
            world,
            instance_root_node,
            peid,
            &type_path,
            &field_path,
            value,
        );
    }
}

/// Write a prefab's cached document to disk and record the saved fingerprint so
/// the file watcher ignores its own echo.
pub fn save_prefab_to_disk(world: &mut World, prefab_path: &Path) -> std::io::Result<()> {
    let text = {
        let cache = world.resource::<PrefabAstCache>();
        let Some(ast) = cache.get(prefab_path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("prefab not cached: {}", prefab_path.display()),
            ));
        };
        emit_scene(ast)
    };
    // Prefabs persist as BSN text; a legacy `.jsn` path writes the `.bsn`
    // sibling and keeps the old file as a `.jsn.bak` backup.
    let write_path = prefab_bsn_path(prefab_path);
    if write_path != prefab_path {
        back_up_legacy_prefab(prefab_path);
    }
    std::fs::write(&write_path, text)?;

    let fingerprint = crate::prefab::cache::compute_file_fingerprint(&write_path)?;
    world
        .resource_mut::<PrefabAstCache>()
        .record_saved_fingerprint(&write_path, fingerprint);
    Ok(())
}

/// Save the active prefab tab's cached document to its source file.
#[operator(
    id = "prefab.save",
    label = "Save Prefab",
    description = "Write the active prefab tab's document out to its source file.",
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
/// (the prefab to instantiate) plus `pos_x`, `pos_y`, `pos_z`.
#[operator(
    id = "prefab.spawn_instance",
    label = "Spawn Prefab Instance",
    description = "Drop a new instance of the given prefab into the active scene at a world position.",
    allows_undo = true,
    params(
        path(String, doc = "Path to the prefab to instantiate."),
        pos_x(f64, doc = "World-space X position."),
        pos_y(f64, doc = "World-space Y position."),
        pos_z(f64, doc = "World-space Z position."),
    )
)]
pub fn prefab_spawn_instance(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let op = "prefab.spawn_instance";
    let Some(path) = require_str(&params, "path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(x) = require_float(&params, "pos_x", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(y) = require_float(&params, "pos_y", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(z) = require_float(&params, "pos_z", op) else {
        return OperatorResult::Cancelled;
    };
    let pos = Vec3::new(x as f32, y as f32, z as f32);
    commands.queue(move |world: &mut World| {
        spawn_instance(world, &PathBuf::from(path), pos);
    });
    OperatorResult::Finished
}

/// Revert a single component field on a prefab-instance entity back to its
/// inherited prefab value.
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
    let op = "prefab.revert_field";
    let Some(entity) = require_entity(&params, "entity", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = require_str(&params, "type_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = require_str(&params, "field_path", op) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(node) = resolve_ast_key(world, entity, op) else {
            return;
        };
        revert_field(world, node, &type_path, &field_path);
    });
    OperatorResult::Finished
}

/// Revert an entire component on a prefab-instance entity back to the prefab's
/// inherited value.
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
    let op = "prefab.revert_component";
    let Some(entity) = require_entity(&params, "entity", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = require_str(&params, "type_path", op) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(node) = resolve_ast_key(world, entity, op) else {
            return;
        };
        revert_component(world, node, &type_path);
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
    let op = "prefab.revert_all";
    let Some(instance_entity) = require_entity(&params, "instance_entity", op) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(node) = resolve_ast_key(world, instance_entity, op) else {
            return;
        };
        revert_all(world, node);
    });
    OperatorResult::Finished
}

/// Apply a single field's scene-side value into the prefab source document so
/// the override becomes the new inherited base. The value is supplied as a
/// JSON-encoded string via `value_json`.
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
    let op = "prefab.apply_to_source";
    let Some(instance_entity) = require_entity(&params, "instance_entity", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(entity_id) = require_int(&params, "entity_id", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = require_str(&params, "type_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = require_str(&params, "field_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(value_json) = require_str(&params, "value_json", op) else {
        return OperatorResult::Cancelled;
    };
    let value: serde_json::Value = match serde_json::from_str(&value_json) {
        Ok(v) => v,
        Err(err) => {
            warn!("{op}: bad `value_json`: {err}");
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| {
        let Some(instance_root) = resolve_ast_key(world, instance_entity, op) else {
            return;
        };
        apply_to_prefab_source(
            world,
            instance_root,
            entity_id as u32,
            &type_path,
            &field_path,
            json_to_bsn_value(&value),
        );
    });
    OperatorResult::Finished
}

/// Apply a single-field delta to every prefab instance in the scene that points
/// at `source_path`.
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
    let op = "prefab.bulk_apply_in_scene";
    let Some(source_path) = require_str(&params, "source_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = require_str(&params, "type_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = require_str(&params, "field_path", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(value_json) = require_str(&params, "value_json", op) else {
        return OperatorResult::Cancelled;
    };
    let value: serde_json::Value = match serde_json::from_str(&value_json) {
        Ok(v) => v,
        Err(err) => {
            warn!("{op}: bad `value_json`: {err}");
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| {
        bulk_apply_in_scene(
            world,
            &PathBuf::from(source_path),
            &type_path,
            &field_path,
            json_to_bsn_value(&value),
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
    let op = "prefab.save_as_variant_entity";
    let Some(bits) = require_int(&params, "instance_root_entity", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(target_path) = require_str(&params, "target_path", op) else {
        return OperatorResult::Cancelled;
    };
    let entity = Entity::from_bits(bits as u64);
    commands.queue(move |world: &mut World| {
        save_as_variant(world, entity, &PathBuf::from(target_path));
    });
    OperatorResult::Finished
}

/// Pop an inherited child out of its prefab instance and re-parent it under
/// another entity in the scene as a standalone (non-instance) entity.
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
    let op = "prefab.unpack_child";
    let Some(child_entity) = require_entity(&params, "child_entity", op) else {
        return OperatorResult::Cancelled;
    };
    let Some(drop_target_entity) = require_entity(&params, "drop_target_entity", op) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(child_node) = resolve_ast_key(world, child_entity, op) else {
            return;
        };
        let Some(drop_target_node) = resolve_ast_key(world, drop_target_entity, op) else {
            return;
        };
        unpack_child(world, child_node, drop_target_node);
    });
    OperatorResult::Finished
}

/// Remove a prefab instance wrapper, promoting its inherited children to the
/// instance's parent slot.
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
    let op = "prefab.unbundle_instance";
    let Some(instance_entity) = require_entity(&params, "instance_entity", op) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(node) = resolve_ast_key(world, instance_entity, op) else {
            return;
        };
        unbundle_instance(world, node);
    });
    OperatorResult::Finished
}

/// Walk every cached prefab document and strip `IsA` components whose `source`
/// resolves back to the prefab itself. Self-referencing `IsA` entries are a
/// poisoned state produced by older save paths; the resolver fails on them.
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
        let to_strip: Vec<Entity> = {
            let cache = world.resource::<PrefabAstCache>();
            let Some(ast) = cache.get(&path) else {
                continue;
            };
            let mut nodes: Vec<Entity> = ast.roots.clone();
            for &root in &ast.roots {
                nodes.extend(ast.descendants_of(root));
            }
            nodes
                .into_iter()
                .filter(|&node| {
                    read_isa_source(ast, node)
                        .map(|src| crate::prefab::canonical_prefab_path(&src) == canonical_target)
                        .unwrap_or(false)
                })
                .collect()
        };
        if to_strip.is_empty() {
            continue;
        }
        world.resource_mut::<PrefabAstCache>().mutate(&path, |ast| {
            for &node in &to_strip {
                ast.remove_component_patch(node, ISA_TYPE);
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

/// Structural conversion of a `serde_json::Value` into a `BsnValue` for the
/// `value_json` operator params. These carry a single field value applied as a
/// delta; scalars map cleanly. Nested objects become anonymous maps: their
/// component type paths are not recoverable from JSON, so a struct-shaped value
/// (e.g. a whole Vec3 rather than a single axis) does not round-trip through
/// emit. Callers pass scalar leaves (dotted `field_path` + scalar), which is
/// the supported case.
fn json_to_bsn_value(value: &serde_json::Value) -> BsnValue {
    match value {
        serde_json::Value::Null => BsnValue::List(Vec::new()),
        serde_json::Value::Bool(b) => BsnValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                BsnValue::Int(i128::from(i))
            } else {
                BsnValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => BsnValue::String(s.clone()),
        serde_json::Value::Array(items) => {
            BsnValue::List(items.iter().map(json_to_bsn_value).collect())
        }
        serde_json::Value::Object(map) => BsnValue::Map(
            map.iter()
                .map(|(k, v)| (BsnValue::String(k.clone()), json_to_bsn_value(v)))
                .collect(),
        ),
    }
}
