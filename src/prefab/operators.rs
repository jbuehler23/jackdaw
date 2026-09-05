//! User-facing prefab operators. Each function mutates world state
//! directly; UI hookups route to these via the operator system.

use crate::prefab::cache::PrefabAstCache;
use crate::prefab::resolver_bsn::{
    isa_value, read_isa_deleted, read_isa_source, read_prefab_entity_id, set_whole_component,
    value_to_patch,
};
use bevy::ecs::hierarchy::{ChildOf, Children};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_bsn::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnValue, SceneBsnAst, emit_scene,
    get_bsn_field, patch_type_path, set_bsn_field,
};
use jackdaw_prefab::source::{peid_value, synthetic_root_patches};
use std::path::{Path, PathBuf};

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";

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
/// A `PrefabEntityId(id)` tuple-struct patch.
fn peid_patch(id: u32) -> BsnPatch {
    value_to_patch(peid_value(id)).expect("PrefabEntityId is a tuple struct")
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

/// A `Transform` struct patch carrying only `translation` (a sparse delta the
/// resolver merges onto the inherited baseline).
fn transform_translation_patch(pos: Vec3) -> BsnPatch {
    BsnPatch::Struct(BsnStructData {
        type_path: TRANSFORM_TYPE.to_string(),
        fields: BsnStructFields(vec![field("translation", vec3_value(pos.x, pos.y, pos.z))]),
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

/// A whole `Transform` struct patch. An instance that stands in for a group
/// takes over its rotation and scale as well as its position, so the delta it
/// carries names all three.
fn transform_patch(transform: Transform) -> BsnPatch {
    let (t, r, s) = (transform.translation, transform.rotation, transform.scale);
    BsnPatch::Struct(BsnStructData {
        type_path: TRANSFORM_TYPE.to_string(),
        fields: BsnStructFields(vec![
            field("translation", vec3_value(t.x, t.y, t.z)),
            field("rotation", quat_value(r.x, r.y, r.z, r.w)),
            field("scale", vec3_value(s.x, s.y, s.z)),
        ]),
    })
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
///
/// A prefab can carry an `IsA` of its own (a variant is a `Prefab` root
/// pointing at the base it was cut from), so the reference is written relative
/// to the file being written, as at any disk boundary. Done on a clone, so the
/// caller's document, and the cache entry it usually becomes, keeps absolute
/// paths.
/// `replace` says whether a file already at the path may be written over.
/// A refusal is the open itself rather than a prior `exists()`: a prefab
/// other scenes instance is not something to lose to whoever wrote it
/// between the check and the write.
fn write_prefab_doc(
    target_path: &Path,
    prefab: &SceneBsnAst,
    op_id: &str,
    replace: bool,
) -> Option<PathBuf> {
    let path = prefab_bsn_path(target_path);
    if path != target_path {
        back_up_legacy_prefab(target_path);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut out = crate::prefab::resolver_bsn::clone_scene(prefab);
    jackdaw_prefab::relativize_isa_sources(&mut out, path.parent().unwrap_or(Path::new("")));
    let text = emit_scene(&out);
    let written = if replace {
        std::fs::write(&path, text)
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()))
    };
    if let Err(err) = written {
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

    save_selection_as_new_prefab(world, &normalized, target_path, true);
}

/// Bundle path: write a fresh prefab file from the selection and replace it
/// in the source scene with an instance of the new prefab.
fn save_selection_as_new_prefab(
    world: &mut World,
    normalized: &[Entity],
    target_path: &Path,
    replace: bool,
) {
    let Some(packed) = write_prefab_from_roots(world, normalized, target_path, replace) else {
        return;
    };
    remove_packed_from_document(world, &packed.entities);
    // spawn_instance adds the instance node and triggers a reload that
    // materializes the inherited children from the prefab we just wrote.
    spawn_instance(world, &packed.path, packed.centroid);
}

/// A prefab file that was just written, and what went into it.
struct PackedPrefab {
    /// The file written, which is where an instance inherits from.
    path: PathBuf,
    /// The centroid the packaged roots were shifted around, which is where
    /// an instance of the file stands for the scene to look as it did.
    centroid: Vec3,
    /// The live-document entities that went into the file. Still in the
    /// document: dropping them is [`remove_packed_from_document`], which a
    /// caller runs once it is satisfied the file it just wrote reads back.
    entities: Vec<Entity>,
}

/// Drop the packaged entities from the live document so the upcoming
/// reload does not respawn them alongside the new instance.
fn remove_packed_from_document(world: &mut World, entities: &[Entity]) {
    let mut live = world.resource_mut::<SceneBsnAst>();
    for &entity in entities {
        live.remove_entity_node(entity);
    }
}

/// Write `normalized` and their descendants out as a prefab file, leaving
/// them in the live document for [`remove_packed_from_document`].
fn write_prefab_from_roots(
    world: &mut World,
    normalized: &[Entity],
    target_path: &Path,
    replace: bool,
) -> Option<PackedPrefab> {
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

    let path = write_prefab_doc(
        target_path,
        &prefab,
        "save_as_prefab_from_selection",
        replace,
    )?;

    Some(PackedPrefab {
        path,
        centroid,
        entities,
    })
}

/// How [`pack_matching_groups`] decides another top-level group is a copy of
/// the one being packed.
pub(crate) enum GroupMatch {
    /// The same subtree: every descendant's identity and local placement,
    /// to the bottom.
    Structural,
    /// Name starting with the given prefix.
    Prefix(String),
}

/// How far two placements may differ and still count as the same.
///
/// Relative to the distance involved, so it means the same thing for a
/// child sitting 0.2m from its parent as for a group standing 5km out:
/// an absolute millimetre is below float precision at that range, and
/// every copy of a distant group would read as different.
const MATCH_TOLERANCE: f32 = 1e-3;

/// Whether two placements are the same within [`MATCH_TOLERANCE`].
fn placements_match(a: &Transform, b: &Transform) -> bool {
    let scaled = |a: Vec3, b: Vec3| {
        let span = a.abs().max(b.abs()).max_element().max(1.0);
        a.abs_diff_eq(b, MATCH_TOLERANCE * span)
    };
    scaled(a.translation, b.translation)
        && a.rotation.abs_diff_eq(b.rotation, MATCH_TOLERANCE)
        && scaled(a.scale, b.scale)
}

/// One node of a group's shape: what the entity is, where it sits in its
/// parent, and the same for its own children in order.
struct SubtreeSignature {
    /// The glTF file the entity draws, or the sorted type paths of the
    /// components it carries when it draws none. A group of two lights
    /// and a group of two empties both have no source; without the
    /// component set standing in for one they would read as the same
    /// shape.
    identity: Vec<String>,
    at: Transform,
    children: Vec<SubtreeSignature>,
}

/// A group's shape, to the bottom of its subtree. Editor-internal
/// children are left out, as they are when the group is packaged.
///
/// The whole subtree, because [`pack_matching_groups`] deletes what it
/// matches: two groups that agree on their direct children and differ
/// below are not copies, and calling them copies drops the chimney off
/// one of the roofs.
fn group_signature(world: &World, root: Entity) -> Vec<SubtreeSignature> {
    let Some(children) = world.get::<Children>(root) else {
        return Vec::new();
    };
    let mut signature = Vec::new();
    for child in children.iter() {
        if world.get::<crate::EditorHidden>(child).is_some()
            || world.get::<crate::NonSerializable>(child).is_some()
        {
            continue;
        }
        signature.push(SubtreeSignature {
            identity: node_identity(world, child),
            at: world.get::<Transform>(child).copied().unwrap_or_default(),
            children: group_signature(world, child),
        });
    }
    signature
}

/// What an entity is, for matching: its glTF source, or the component
/// types it carries when it has none.
fn node_identity(world: &World, entity: Entity) -> Vec<String> {
    if let Some(source) = world.get::<crate::entity_ops::GltfSource>(entity) {
        return vec![source.path.clone()];
    }
    let Ok(components) = world.inspect_entity(entity) else {
        return Vec::new();
    };
    let mut types: Vec<String> = components
        .filter(|info| info.type_id().is_some())
        .map(|info| info.name().to_string())
        .collect();
    types.sort();
    types
}

fn signatures_match(a: &[SubtreeSignature], b: &[SubtreeSignature]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.identity == b.identity
                && placements_match(&a.at, &b.at)
                && signatures_match(&a.children, &b.children)
        })
}

/// The scene's top-level entities, in document order.
fn top_level_entities(world: &World) -> Vec<Entity> {
    let live = world.resource::<SceneBsnAst>();
    live.roots
        .iter()
        .filter_map(|&node| live.ecs_for_ast(node))
        .collect()
}

/// The group an operator acts on: its `entity` parameter, or the primary
/// selection when that was left out.
fn target_group(world: &World, entity: Option<Entity>, op_id: &str) -> Option<Entity> {
    let target = entity.or_else(|| world.resource::<crate::selection::Selection>().primary());
    if target.is_none() {
        warn!("{op_id}: no `entity` param and nothing is selected");
    }
    target
}

/// Resolve a caller-supplied prefab path against the open project's assets
/// directory, refusing one that would land outside it.
///
/// `path` reaches these operators from a remote caller, and `prefab.pack`
/// writes where it points, so the confinement
/// [`crate::project::path_within`] applies is what makes "relative to the
/// project's assets directory" true rather than only documented. A path
/// that already names `assets/` is taken from the project root, so both
/// spellings reach the same file.
fn resolve_asset_path(world: &mut World, path: &str, op_id: &str) -> Option<PathBuf> {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|project| project.root.clone())
    else {
        warn_caller(world, format!("{op_id}: no project is open"));
        return None;
    };
    let path = Path::new(path);
    let candidate = if path.starts_with("assets") {
        path.to_path_buf()
    } else {
        Path::new("assets").join(path)
    };
    match crate::project::path_within(&root, &candidate) {
        Ok(resolved) => Some(resolved),
        Err(refusal) => {
            warn_caller(world, format!("{op_id}: {refusal}"));
            None
        }
    }
}

/// Whether `target_path` is free to write. A prefab file is a document other
/// scenes may already instance, so replacing one is asked for rather than
/// assumed.
fn target_is_writable(world: &mut World, target_path: &Path, overwrite: bool, op_id: &str) -> bool {
    let path = prefab_bsn_path(target_path);
    if overwrite || !path.exists() {
        return true;
    }
    warn_caller(
        world,
        format!(
            "{op_id}: {} already exists; pass overwrite=true to replace it",
            path.display()
        ),
    );
    false
}

/// Drop any cached copy of `target_path`, so what gets instanced is the file
/// just written rather than the document an earlier pack left behind.
fn forget_cached_prefab(world: &mut World, target_path: &Path) {
    let path = prefab_bsn_path(target_path);
    world.resource_mut::<PrefabAstCache>().invalidate(&path);
}

/// Drop `root` and its descendants from the live document. The ECS entities go
/// with the respawn that follows.
fn remove_subtree_from_document(world: &mut World, root: Entity) {
    let mut entities = vec![root];
    collect_descendants(world, root, &mut entities);
    let mut live = world.resource_mut::<SceneBsnAst>();
    for entity in entities {
        live.remove_entity_node(entity);
    }
}

/// Record the last `count` document roots as entities this call added.
///
/// An instance goes into the document as a node and the scene is rebuilt
/// from it, which mints a fresh id for every entity in the document. So
/// the id worth reporting is the one the rebuild left standing, read back
/// after it from the position the node went in at, rather than the one
/// that existed while the node was being written.
fn record_spawned_roots(world: &mut World, count: usize) {
    let added: Vec<Entity> = {
        let live = world.resource::<SceneBsnAst>();
        live.roots
            .iter()
            .rev()
            .take(count)
            .filter_map(|&node| live.ecs_for_ast(node))
            .collect()
    };
    for entity in added.into_iter().rev() {
        crate::commands::SpawnedEntities::record(world, entity);
    }
}

/// Add an instance root for `source` carrying `transform`, without the respawn
/// [`spawn_instance`] does; a caller adding several reloads once for all of
/// them.
fn add_instance_node(world: &mut World, source: &Path, transform: Transform) {
    let patches = vec![
        isa_patch(&source.to_string_lossy(), &[]),
        peid_patch(0),
        transform_patch(transform),
    ];
    let mut live = world.resource_mut::<SceneBsnAst>();
    let node = live.create_entity_node(patches);
    live.add_to_roots(node);
}

/// The scale an instance carries to stand at `target`'s size, given that
/// the prefab already holds `packed`'s own scale.
///
/// `None` when the ratio is not the same on every axis. The instance
/// applies its scale under its rotation, so an uneven ratio composed with
/// a rotation is a shear, not the placement the group had, and
/// [`pack_matching_groups`] leaves such a group alone rather than move it
/// somewhere it never stood.
fn instance_scale(packed: Vec3, target: Vec3) -> Option<Vec3> {
    let ratio = |target: f32, packed: f32| {
        if packed.abs() > f32::EPSILON {
            target / packed
        } else {
            target
        }
    };
    let scale = Vec3::new(
        ratio(target.x, packed.x),
        ratio(target.y, packed.y),
        ratio(target.z, packed.z),
    );
    let even = (scale.max_element() - scale.min_element()).abs()
        <= MATCH_TOLERANCE * scale.abs().max_element().max(1.0);
    even.then_some(scale)
}

/// The transform an instance carries to stand where `target` stood, given that
/// the prefab already holds `packed`'s own rotation and scale.
fn instance_delta(packed: Transform, target: Transform) -> Option<Transform> {
    Some(Transform {
        translation: target.translation,
        rotation: target.rotation * packed.rotation.inverse(),
        scale: instance_scale(packed.scale, target.scale)?,
    })
}

/// Pack `root` and its subtree into a prefab file, replacing it in the scene
/// with an instance standing where the group stood.
pub(crate) fn pack_group(
    world: &mut World,
    root: Entity,
    target_path: &Path,
    overwrite: bool,
) -> bool {
    if !target_is_writable(world, target_path, overwrite, "prefab.pack") {
        return false;
    }
    forget_cached_prefab(world, target_path);
    save_selection_as_new_prefab(world, &[root], target_path, overwrite);
    true
}

/// Pack `root` as [`pack_group`] does, then replace every other top-level
/// group `matcher` accepts with an instance of the same file, each keeping the
/// transform its group had. Returns how many groups became instances, `root`
/// included.
pub(crate) fn pack_matching_groups(
    world: &mut World,
    root: Entity,
    target_path: &Path,
    matcher: &GroupMatch,
    overwrite: bool,
) -> Option<usize> {
    let op = "prefab.pack_matching";
    if !target_is_writable(world, target_path, overwrite, op) {
        return None;
    }
    let packed = world.get::<Transform>(root).copied().unwrap_or_default();
    let signature = group_signature(world, root);
    let mut matched: Vec<(Entity, Transform)> = Vec::new();
    for entity in top_level_entities(world) {
        if entity == root {
            continue;
        }
        let accepted = match matcher {
            GroupMatch::Structural => signatures_match(&signature, &group_signature(world, entity)),
            GroupMatch::Prefix(prefix) => world
                .get::<Name>(entity)
                .is_some_and(|name| name.as_str().starts_with(prefix.as_str())),
        };
        if !accepted {
            continue;
        }
        let at = world.get::<Transform>(entity).copied().unwrap_or_default();
        let Some(delta) = instance_delta(packed, at) else {
            let name = world
                .get::<Name>(entity)
                .map_or_else(|| format!("{entity}"), |name| name.as_str().to_string());
            warn_caller(
                world,
                format!("{op}: {name} is scaled unevenly against the packed group; left alone"),
            );
            continue;
        };
        matched.push((entity, delta));
    }

    forget_cached_prefab(world, target_path);
    let written = write_prefab_from_roots(world, &[root], target_path, overwrite)?;
    // The instances go in as document nodes rather than through
    // `spawn_instance`, so the file has to be in the cache before the one
    // respawn at the end resolves them.
    crate::prefab::save_load::cache_prefab_tree(
        &written.path,
        &mut world.resource_mut::<PrefabAstCache>(),
    );
    // Nothing comes out of the document until the file it would inherit
    // from is known to read back: the alternative is a scene with the
    // group gone and no instance standing where it was.
    if world
        .resource::<PrefabAstCache>()
        .get(&written.path)
        .is_none()
    {
        warn_caller(
            world,
            format!("{op}: failed to read back {}", written.path.display()),
        );
        return None;
    }

    remove_packed_from_document(world, &written.entities);
    add_instance_node(
        world,
        &written.path,
        Transform::from_translation(written.centroid),
    );
    for (entity, delta) in &matched {
        remove_subtree_from_document(world, *entity);
        add_instance_node(world, &written.path, *delta);
    }
    crate::prefab::watcher::reload_all_instances(world);
    record_spawned_roots(world, matched.len() + 1);
    Some(matched.len() + 1)
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

    let Some(target_path) =
        write_prefab_doc(target_path, &prefab, "propagate_instance_to_prefab", true)
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

/// Pending prefab file-pick dialog for `entity.add.prefab`.
///
/// The dialog is asynchronous because `pick_file` blocks until the user
/// answers. The operator returns as soon as the dialog is up, and
/// [`poll_prefab_pick`] spawns the instance when the answer arrives.
#[derive(Resource)]
pub struct PrefabPickTask(bevy::tasks::Task<Option<rfd::FileHandle>>);

/// Open the prefab file picker backing `entity.add.prefab`. No-op while a
/// pick is already pending.
pub fn open_prefab_picker(world: &mut World) {
    use bevy::window::{PrimaryWindow, RawHandleWrapper};

    if world.contains_resource::<PrefabPickTask>() {
        return;
    }
    let raw_handle = world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned();
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Select prefab")
        .add_filter("Prefab", &["bsn"]);
    // Where `Save As Prefab` writes them, when a project is open.
    if let Some(root) = world.get_resource::<crate::project::ProjectRoot>() {
        let prefabs = root.root.join("assets/prefabs");
        if prefabs.is_dir() {
            dialog = dialog.set_directory(prefabs);
        }
    }
    if let Some(ref handle) = raw_handle {
        // SAFETY: called on the main thread from an exclusive context
        let handle = unsafe { handle.get_handle() };
        dialog = dialog.set_parent(&handle);
    }
    let task =
        bevy::tasks::AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(PrefabPickTask(task));
}

/// Spawn the instance once the picker has an answer.
pub fn poll_prefab_pick(world: &mut World) {
    use bevy::tasks::futures_lite::future;

    let Some(mut task) = world.get_resource_mut::<PrefabPickTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    world.remove_resource::<PrefabPickTask>();
    let Some(file_handle) = result else {
        return;
    };
    spawn_instance(world, file_handle.path(), Vec3::ZERO);
}

/// Add a new prefab instance to the live scene at `world_pos`. Caches the
/// prefab document if missing, adds an instance root carrying
/// `IsA + PrefabEntityId + Transform` (a translation-only sparse delta), then
/// resolves + respawns the scene preview.
///
/// Importing a UI scene into a world scene goes through this same call: a UI
/// scene is an ordinary `.bsn`, and `read_prefab_ast` marks a single-rooted
/// document as a prefab source in place, so the instance root inherits the
/// authored `UiSceneRoot` and remains a valid Bevy UI root. Only the placement
/// differs; see `source_root_is_ui_scene`.
pub fn spawn_instance(world: &mut World, prefab_path: &Path, world_pos: Vec3) {
    // Caches the prefab's own `IsA` ancestry alongside it, without which a
    // two-level prefab resolves to nothing. Cached entries are left untouched,
    // so a second spawn reuses the first one's copy.
    crate::prefab::save_load::cache_prefab_tree(
        prefab_path,
        &mut world.resource_mut::<PrefabAstCache>(),
    );
    if world
        .resource::<PrefabAstCache>()
        .get(prefab_path)
        .is_none()
    {
        warn!(
            "spawn_instance: failed to read prefab {}",
            prefab_path.display()
        );
        return;
    }

    let ui_scene = world
        .resource::<PrefabAstCache>()
        .get(prefab_path)
        .is_some_and(source_root_is_ui_scene);

    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let source = prefab_path.to_string_lossy().into_owned();
        let mut patches = vec![isa_patch(&source, &[]), peid_patch(0)];
        if !ui_scene {
            patches.push(transform_translation_patch(world_pos));
        }
        let node = live.create_entity_node(patches);
        live.add_to_roots(node);
    }

    crate::prefab::watcher::reload_all_instances(world);
    record_spawned_roots(world, 1);
}

/// Whether instancing this source produces a UI scene root.
///
/// Asked of the source's root rather than of the whole document, since only the
/// root's components are inherited onto the instance: a world scene with a UI
/// overlay in a subtree instances as an ordinary prefab, with its overlay still
/// a descendant.
///
/// Callers use this to decide placement. A 3D instance is placed by a
/// `Transform` delta; a UI root is placed by layout against its target camera's
/// viewport, so a `Transform` authored on one would be a scene-file component
/// nothing reads.
fn source_root_is_ui_scene(prefab: &SceneBsnAst) -> bool {
    prefab.roots.first().is_some_and(|&root| {
        prefab
            .component_type_paths(root)
            .iter()
            .any(|type_path| crate::scene_io::is_ui_scene_root_type_path(type_path))
    })
}

/// Open the scene an instance root inherits from, in its own tab.
///
/// Bound to double-clicking a UI prefab instance, whose imported overlay is
/// read-only in place and is edited through its source file.
/// `scene_open_system` de-dupes against the open tabs, so a second double-click
/// activates the tab the first one opened.
///
/// Returns whether a source was found and opened. A missing file warns rather
/// than opening an empty tab, since an instance whose source moved is still
/// resolvable from the prefab cache.
pub fn open_instance_source(world: &mut World, instance_root: Entity) -> bool {
    let Some(source) = world
        .get::<crate::prefab::IsA>(instance_root)
        .map(|isa| isa.source.clone())
    else {
        return false;
    };
    let scene_dir = world
        .resource::<crate::scene_io::SceneFilePath>()
        .path
        .as_ref()
        .map(PathBuf::from)
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = crate::prefab::save_load::resolve_source_path(&source, &scene_dir);
    if !path.exists() {
        warn!(
            "prefab.open_source: instance source {} is not on disk",
            path.display()
        );
        return false;
    }
    crate::scenes::operators::scene_open_system(world, &path);
    true
}

/// The prefab document a baseline lookup must read, with the prefab's own
/// `IsA` chain already expanded.
///
/// The raw file carries only the ids the prefab itself authors. Ids inherited
/// through that prefab, such as a two-level prefab's base subtree, exist only
/// once the source is resolved, so a baseline looked up in the unexpanded
/// document misses every transitively inherited id.
fn expanded_prefab_source(world: &World, source: &Path) -> Option<SceneBsnAst> {
    let cache = world.resource::<PrefabAstCache>();
    let raw = cache.get(source)?;
    let get_prefab = |p: &Path| cache.get(p);
    match crate::prefab::resolver_bsn::resolve_scene(raw, &get_prefab) {
        Ok(expanded) => Some(expanded),
        Err(e) => {
            warn!(
                "prefab baseline: resolving source {} failed: {e}",
                source.display()
            );
            None
        }
    }
}

/// The `IsA` source `node` inherits from, if any.
fn instance_source_of(world: &World, node: Entity) -> Option<PathBuf> {
    let live = world.resource::<SceneBsnAst>();
    let isa_node = live.ancestor_with_component(node, ISA_TYPE)?;
    read_isa_source(live, isa_node)
}

/// Cache the source's own `IsA` ancestry before a baseline is resolved against
/// it.
///
/// A prefab cached by a route that read only the file it names has none of its
/// ancestry behind it, and the resolver refuses a document whose chain it
/// cannot follow rather than expanding part of it, which would make a
/// revertable nested instance read as unresolvable.
fn prime_source_ancestry(world: &mut World, source: &Path) {
    crate::prefab::save_load::cache_prefab_tree(
        source,
        &mut world.resource_mut::<PrefabAstCache>(),
    );
}

/// The prefab baseline value (whole component when `field_path` is empty, else
/// the leaf at `field_path`) for the entity's `(source, PrefabEntityId)`.
fn resolve_prefab_value(
    world: &World,
    node: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<BsnValue> {
    let (peid, source) = {
        let live = world.resource::<SceneBsnAst>();
        let peid = read_prefab_entity_id(live, node)?;
        let isa_node = live.ancestor_with_component(node, ISA_TYPE)?;
        (peid, read_isa_source(live, isa_node)?)
    };
    let prefab = expanded_prefab_source(world, &source)?;
    let prefab_node = prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))?;
    get_bsn_field(&prefab, prefab_node, type_path, field_path)
}

/// Revert one component field on a prefab-instance entity to its inherited
/// value. After mutating the document, respawns so the live preview reflects
/// the revert. Returns whether anything was reverted; a target with no
/// resolvable baseline is reported rather than dropped.
pub fn revert_field(world: &mut World, node: Entity, type_path: &str, field_path: &str) -> bool {
    if let Some(source) = instance_source_of(world, node) {
        prime_source_ancestry(world, &source);
    }
    let Some(prefab_leaf) = resolve_prefab_value(world, node, type_path, field_path) else {
        warn!(
            "revert_field: no prefab baseline for node={node:?} type_path={type_path} \
             field_path={field_path}; nothing was reverted"
        );
        return false;
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    {
        let reg = registry.read();
        let mut live = world.resource_mut::<SceneBsnAst>();
        if get_bsn_field(&live, node, type_path, "").is_none() {
            return false;
        }
        set_bsn_field(&mut live, node, type_path, field_path, prefab_leaf, &reg);
    }
    crate::prefab::watcher::reload_all_instances(world);
    true
}

/// Revert an entire component on a prefab-instance entity to the prefab's
/// value. Bails if there is no resolvable prefab inheritance for the component;
/// removing in that case would silently destroy authored data.
pub fn revert_component(world: &mut World, node: Entity, type_path: &str) {
    if let Some(source) = instance_source_of(world, node) {
        prime_source_ancestry(world, &source);
    }
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

/// The outcome of a revert pass. `unresolved` counts instance entities whose
/// `PrefabEntityId` had no counterpart in the resolved source and were left as
/// authored, so a pass with a non-zero `unresolved` is not a complete revert.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RevertReport {
    pub reverted: usize,
    pub unresolved: usize,
}

/// Revert every override on an instance subtree. Walks the root and its
/// descendants; for each that has a `PrefabEntityId`, resets all non-marker
/// components to the prefab baseline while preserving `IsA` + `PrefabEntityId`.
pub fn revert_all(world: &mut World, instance_root_node: Entity) -> RevertReport {
    let source = {
        let live = world.resource::<SceneBsnAst>();
        let found = read_isa_source(live, instance_root_node);
        let Some(source) = found else {
            warn!(
                "revert_all: node={instance_root_node:?} is not a prefab instance root; \
                 nothing was reverted"
            );
            crate::status_bar::notify_error(
                world,
                "Nothing reverted: this is not a prefab instance".to_string(),
            );
            return RevertReport::default();
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

    // Expand the source once and reuse it for every target. The expansion is
    // what makes a nested prefab's inherited ids findable; doing it per node
    // would re-resolve the whole `IsA` chain for each entity in the subtree.
    // Baselines are read while borrowing the cache immutably and applied
    // afterwards, so the cache is never cloned.
    prime_source_ancestry(world, &source);
    let expanded = expanded_prefab_source(world, &source);
    let Some(prefab) = expanded else {
        warn!(
            "revert_all: prefab source {} did not resolve; nothing was reverted",
            source.display()
        );
        crate::status_bar::notify_error(
            world,
            format!(
                "Nothing reverted: prefab source {} did not resolve",
                source.display()
            ),
        );
        return RevertReport::default();
    };

    let mut unresolved: Vec<u32> = Vec::new();
    let baselines: Vec<(Entity, Vec<(String, BsnValue)>)> = {
        let live = world.resource::<SceneBsnAst>();
        let mut baselines = Vec::new();
        for node in targets {
            let Some(peid) = read_prefab_entity_id(live, node) else {
                continue;
            };
            let Some(prefab_node) =
                prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))
            else {
                unresolved.push(peid);
                continue;
            };
            let comps: Vec<(String, BsnValue)> = prefab
                .component_type_paths(prefab_node)
                .into_iter()
                .filter(|tp| tp != PREFAB_TYPE)
                .filter_map(|tp| get_bsn_field(&prefab, prefab_node, &tp, "").map(|v| (tp, v)))
                .collect();
            baselines.push((node, comps));
        }
        baselines
    };
    let report = RevertReport {
        reverted: baselines.len(),
        unresolved: unresolved.len(),
    };
    if !unresolved.is_empty() {
        warn!(
            "revert_all: {} of {} entities have no baseline in {} (PrefabEntityId \
             {unresolved:?}) and were left as authored",
            report.unresolved,
            report.reverted + report.unresolved,
            source.display()
        );
        // A partial "Revert All" is reported: the remaining entities keep
        // their overrides with no other visible indication.
        crate::status_bar::notify_warn(
            world,
            format!(
                "Reverted {} of {} entities; {} had no baseline in the prefab",
                report.reverted,
                report.reverted + report.unresolved,
                report.unresolved
            ),
        );
    }

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
    report
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
        BsnPatch::Type(PREFAB_TYPE.to_string()),
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

    let Some(target_path) = write_prefab_doc(target_path, &variant, "save_as_variant", true) else {
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
    // Prefabs persist as BSN text; a legacy `.jsn` path writes the `.bsn`
    // sibling and keeps the old file as a `.jsn.bak` backup.
    let write_path = prefab_bsn_path(prefab_path);
    let text = {
        let cache = world.resource::<PrefabAstCache>();
        let Some(ast) = cache.get(prefab_path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("prefab not cached: {}", prefab_path.display()),
            ));
        };
        // A prefab that instances another names it relative to itself, as a
        // scene does. Rewritten on the copy being written, so the cached
        // document keeps absolute paths.
        let mut out = crate::prefab::resolver_bsn::clone_scene(ast);
        jackdaw_prefab::relativize_isa_sources(
            &mut out,
            write_path.parent().unwrap_or(Path::new("")),
        );
        emit_scene(&out)
    };
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

/// Open the scene a prefab instance inherits from, in its own tab.
#[operator(
    id = "prefab.open_source",
    label = "Open Prefab Source",
    description = "Open the scene a prefab instance inherits from in its own tab.",
    allows_undo = false,
    params(entity(Entity, doc = "ECS entity of the instance root."))
)]
pub fn prefab_open_source(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = require_entity(&params, "entity", "prefab.open_source") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        open_instance_source(world, entity);
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
        path(
            String,
            doc = "Prefab to instantiate. A relative path resolves under the \
                   project's assets directory."
        ),
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
        let Some(path) = resolve_asset_path(world, &path, op) else {
            return;
        };
        spawn_instance(world, &path, pos);
    });
    OperatorResult::Finished
}

/// Turn a group into a prefab file and an instance of it.
#[operator(
    id = "prefab.pack",
    label = "Pack Group as Prefab",
    description = "Write a group out as a prefab file and replace it with an instance of that file.",
    allows_undo = true,
    params(
        entity(Entity, doc = "Group to pack. Defaults to the selection."),
        path(
            String,
            doc = "Where to write the prefab, relative to the project's assets directory."
        ),
        overwrite(
            bool,
            doc = "Replace an existing file at that path. Refused without it."
        ),
    )
)]
pub fn prefab_pack(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let op = "prefab.pack";
    let Some(path) = require_str(&params, "path", op) else {
        return OperatorResult::Cancelled;
    };
    let entity = params.as_entity("entity");
    let overwrite = params.as_bool("overwrite").unwrap_or(false);
    commands.queue(move |world: &mut World| {
        let Some(root) = target_group(world, entity, op) else {
            return;
        };
        let Some(target) = resolve_asset_path(world, &path, op) else {
            return;
        };
        pack_group(world, root, &target, overwrite);
    });
    OperatorResult::Finished
}

/// Pack one group and replace its copies elsewhere in the scene with
/// instances of the same file.
#[operator(
    id = "prefab.pack_matching",
    label = "Pack Matching Groups as Prefab",
    description = "Pack a group as a prefab file, then replace every other top-level group that \
                   matches it with an instance of that file.",
    allows_undo = true,
    params(
        entity(Entity, doc = "Group to pack. Defaults to the selection."),
        path(
            String,
            doc = "Where to write the prefab, relative to the project's assets directory."
        ),
        r#match(
            String,
            doc = "`structural` (the default) compares each group's child glTF sources and local \
                   transforms; `prefix` compares names against `prefix`."
        ),
        prefix(String, doc = "Name prefix, for `match=prefix`."),
        overwrite(
            bool,
            doc = "Replace an existing file at that path. Refused without it."
        ),
    )
)]
pub fn prefab_pack_matching(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let op = "prefab.pack_matching";
    let Some(path) = require_str(&params, "path", op) else {
        return OperatorResult::Cancelled;
    };
    let entity = params.as_entity("entity");
    let overwrite = params.as_bool("overwrite").unwrap_or(false);
    let matcher = match params.as_str("match").unwrap_or("structural") {
        "prefix" => match params.as_str("prefix").filter(|prefix| !prefix.is_empty()) {
            Some(prefix) => GroupMatch::Prefix(prefix.to_string()),
            None => {
                warn!("{op}: match=prefix needs a `prefix` param");
                return OperatorResult::Cancelled;
            }
        },
        "structural" => GroupMatch::Structural,
        other => {
            warn!("{op}: unknown match `{other}`; expected `structural` or `prefix`");
            return OperatorResult::Cancelled;
        }
    };
    commands.queue(move |world: &mut World| {
        let Some(root) = target_group(world, entity, op) else {
            return;
        };
        let Some(target) = resolve_asset_path(world, &path, op) else {
            return;
        };
        if let Some(count) = pack_matching_groups(world, root, &target, &matcher, overwrite) {
            report_to_caller(
                world,
                format!("{op}: replaced {count} groups with instances"),
            );
        }
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
