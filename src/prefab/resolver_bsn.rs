//! Overlay a cached prefab BSN document under an instance node to
//! produce a resolved scene document. Each `IsA` node in the input
//! document gets its inherited subtree materialized, with the instance's
//! sparse field deltas applied on top.
//!
//! Prefabs are supplied through a caller-owned lookup closure rather than
//! the `PrefabAstCache` the JSON resolver reads, so the port does not
//! depend on the cache's element type. The live cutover passes the real
//! cache's getter.

use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use bevy::platform::collections::{HashMap, HashSet};

use jackdaw_bsn::{
    BsnPatch, BsnValue, SceneBsnAst, apply_deltas, bsn_value_as_int, bsn_value_eq, clone_node_into,
    get_bsn_field, shallow_diff,
};

use std::fmt;

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

pub(crate) const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
pub(crate) const PREFAB_ENTITY_ID_TYPE: &str = "jackdaw::prefab::components::PrefabEntityId";
pub(crate) const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

/// A prefab source lookup: maps a project-relative prefab path to its
/// cached BSN document. Tests back this with a `HashMap<PathBuf,
/// SceneBsnAst>`; the live cutover backs it with the prefab cache.
pub type PrefabLookup<'a> = dyn Fn(&Path) -> Option<&'a SceneBsnAst> + 'a;

/// Resolve all `IsA` instances in `scene` against `get_prefab`. Returns a
/// new document where every instance has its inherited subtree
/// materialized, with sparse overrides applied.
pub fn resolve_scene(
    scene: &SceneBsnAst,
    get_prefab: &PrefabLookup,
) -> Result<SceneBsnAst, ResolveError> {
    let mut resolved = clone_scene(scene);
    let chain: Vec<PathBuf> = Vec::new();
    expand_instances(&mut resolved, get_prefab, &chain)?;
    Ok(resolved)
}

/// Expand every `IsA` node in `ast` in place. For each instance the prefab
/// is looked up, deep-cloned, recursively expanded (so nested prefabs
/// materialize first), then merged under the instance node.
fn expand_instances(
    ast: &mut SceneBsnAst,
    get_prefab: &PrefabLookup,
    chain: &[PathBuf],
) -> Result<(), ResolveError> {
    let isa_nodes = ast.entities_with_component(ISA_TYPE);
    for node in isa_nodes {
        let isa_source = read_isa_source(ast, node)
            .ok_or_else(|| ResolveError::BadIsA("IsA.source missing".into()))?;
        let deleted = read_isa_deleted(ast, node);

        if let Some(err) = would_cycle(chain, &isa_source) {
            return Err(ResolveError::Cycle(err));
        }

        let mut prefab_clone = {
            let prefab = get_prefab(&isa_source)
                .ok_or_else(|| ResolveError::PrefabNotCached(isa_source.clone()))?;
            clone_scene(prefab)
        };

        let mut next_chain = chain.to_vec();
        next_chain.push(isa_source.clone());
        expand_instances(&mut prefab_clone, get_prefab, &next_chain)?;

        merge_prefab_under_instance(ast, node, &prefab_clone, &deleted);
    }
    Ok(())
}

/// Overlay `prefab` under `instance_root`: inherit the prefab root's
/// components onto the instance, materialize prefab descendants that the
/// instance does not already carry (skipping `deleted` ids), then merge
/// the instance's sparse overrides onto the inherited baselines.
fn merge_prefab_under_instance(
    ast: &mut SceneBsnAst,
    instance_root: Entity,
    prefab: &SceneBsnAst,
    deleted: &[u32],
) {
    let Some(prefab_root) = prefab
        .roots
        .iter()
        .copied()
        .find(|root| prefab.find_patch_by_type_path(*root, PREFAB_TYPE).is_some())
    else {
        return;
    };

    let existing_ids: HashSet<u32> = ast
        .descendants_of(instance_root)
        .into_iter()
        .filter_map(|node| read_prefab_entity_id(ast, node))
        .collect();

    inherit_root_components(ast, instance_root, prefab, prefab_root);
    materialize_descendants(
        ast,
        instance_root,
        prefab,
        prefab_root,
        deleted,
        &existing_ids,
    );
    merge_descendant_overrides(ast, instance_root, prefab, prefab_root);
}

/// Copy each prefab-root component (except `Prefab` and `IsA`) onto the
/// instance root. When the instance already authors a component, the
/// authored value is treated as a sparse delta applied onto a clone of the
/// inherited base; otherwise the base is inserted whole.
fn inherit_root_components(
    ast: &mut SceneBsnAst,
    instance_root: Entity,
    prefab: &SceneBsnAst,
    prefab_root: Entity,
) {
    let inherited: Vec<(String, BsnValue)> = prefab
        .component_type_paths(prefab_root)
        .into_iter()
        .filter(|tp| tp != PREFAB_TYPE && tp != ISA_TYPE)
        .filter_map(|tp| get_bsn_field(prefab, prefab_root, &tp, "").map(|base| (tp, base)))
        .collect();

    for (type_path, base) in inherited {
        match get_bsn_field(ast, instance_root, &type_path, "") {
            Some(delta) => {
                let mut merged = base;
                apply_deltas(&mut merged, &delta);
                set_whole_component(ast, instance_root, &type_path, merged);
            }
            None => set_whole_component(ast, instance_root, &type_path, base),
        }
    }

    // The prefab root's name is stored as a name-reference patch, not a
    // component patch, so the loop above never copies it. Inherit it when the
    // instance authors no name of its own; scene lifecycle (persistable-root
    // queries, despawn collection) keys off named roots, so an unnamed
    // instance would leak on reload and drop out of saves.
    if ast.get_name(instance_root).is_none()
        && let Some(name) = prefab.get_name(prefab_root).map(str::to_owned)
    {
        let patch_entity = ast.world.spawn(BsnPatch::Name(name)).id();
        if let Some(patches) = ast.get_patches_mut(instance_root) {
            patches.0.push(patch_entity);
        }
    }
}

/// Clone every prefab descendant the instance does not already carry into
/// the scene, keyed by `PrefabEntityId`. Skips ids in `deleted` and ids the
/// instance already provides. A running prefab-node to scene-node map,
/// seeded with the prefab root mapped to the instance root, tracks where
/// each cloned node's parent landed.
fn materialize_descendants(
    ast: &mut SceneBsnAst,
    instance_root: Entity,
    prefab: &SceneBsnAst,
    prefab_root: Entity,
    deleted: &[u32],
    existing_ids: &HashSet<u32>,
) {
    let mut prefab_to_scene: HashMap<Entity, Entity> = HashMap::default();
    prefab_to_scene.insert(prefab_root, instance_root);

    for prefab_node in prefab.descendants_of(prefab_root) {
        let Some(id) = read_prefab_entity_id(prefab, prefab_node) else {
            continue;
        };
        if deleted.contains(&id) || existing_ids.contains(&id) {
            continue;
        }
        let prefab_parent = prefab.ast_parent_of(prefab_node).unwrap_or(prefab_root);
        let scene_parent = prefab_to_scene
            .get(&prefab_parent)
            .copied()
            .unwrap_or(instance_root);
        let new_node = clone_node_into(ast, prefab, prefab_node, scene_parent);
        prefab_to_scene.insert(prefab_node, new_node);
    }
}

/// For every instance descendant carrying a `PrefabEntityId`, fill in the
/// prefab components the override lacks, then merge the components the
/// override does provide onto their inherited baselines as sparse deltas.
fn merge_descendant_overrides(
    ast: &mut SceneBsnAst,
    instance_root: Entity,
    prefab: &SceneBsnAst,
    prefab_root: Entity,
) {
    let override_nodes: Vec<Entity> = ast
        .descendants_of(instance_root)
        .into_iter()
        .filter(|node| {
            ast.find_patch_by_type_path(*node, PREFAB_ENTITY_ID_TYPE)
                .is_some()
        })
        .collect();

    for node in override_nodes {
        let Some(id) = read_prefab_entity_id(ast, node) else {
            continue;
        };
        let Some(prefab_match) = prefab
            .descendants_of(prefab_root)
            .into_iter()
            .find(|pn| read_prefab_entity_id(prefab, *pn) == Some(id))
        else {
            continue;
        };

        // Inherit every prefab component the override does not already
        // provide. Capture strips components matching the baseline; without
        // this fill-in an override materializes carrying only its id.
        let inherited: Vec<(String, BsnValue)> = prefab
            .component_type_paths(prefab_match)
            .into_iter()
            .filter(|tp| tp != PREFAB_ENTITY_ID_TYPE && tp != ISA_TYPE)
            .filter_map(|tp| get_bsn_field(prefab, prefab_match, &tp, "").map(|base| (tp, base)))
            .collect();
        for (type_path, base) in inherited {
            if ast.find_patch_by_type_path(node, &type_path).is_none() {
                set_whole_component(ast, node, &type_path, base);
            }
        }

        // For any component the override does provide, merge it onto the
        // inherited base as a sparse delta.
        let overrides: Vec<(String, BsnValue)> = ast
            .component_type_paths(node)
            .into_iter()
            .filter(|tp| tp != PREFAB_ENTITY_ID_TYPE && tp != ISA_TYPE)
            .filter_map(|tp| get_bsn_field(ast, node, &tp, "").map(|delta| (tp, delta)))
            .collect();
        for (type_path, delta) in overrides {
            if let Some(base) = get_bsn_field(prefab, prefab_match, &type_path, "")
                && !bsn_value_eq(&base, &delta)
            {
                let mut merged = base;
                apply_deltas(&mut merged, &delta);
                set_whole_component(ast, node, &type_path, merged);
            }
        }

        // The name is a reference patch, not a component patch, so the
        // fill-in above never restores it; sparse override entries shed a
        // baseline-matching name at capture, so inherit it back here.
        if ast.get_name(node).is_none()
            && let Some(name) = prefab.get_name(prefab_match).map(str::to_owned)
        {
            let patch_entity = ast.world.spawn(BsnPatch::Name(name)).id();
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.push(patch_entity);
            }
        }
    }
}

/// Reduce inherited descendants of prefab instances to sparse override
/// entries by stripping components that match the matching prefab entry's
/// baseline. A node qualifies when it carries a `PrefabEntityId`, does not
/// carry `IsA`, and has an `IsA` ancestor whose source resolves to a known
/// prefab. `PrefabEntityId` is kept unconditionally.
///
/// This is the capture-path inverse of [`resolve_scene`], mirroring
/// `prefabify_inherited_descendants` in `scene_io`. It retains a whole
/// component whenever it differs from the baseline (not a shallow diff),
/// matching that capture path's semantics.
pub fn sparsify_inherited_descendants(ast: &mut SceneBsnAst, get_prefab: &PrefabLookup) {
    let mut nodes: Vec<Entity> = Vec::new();
    for &root in &ast.roots {
        nodes.push(root);
        nodes.extend(ast.descendants_of(root));
    }

    for node in nodes {
        // Instance roots sparsify against their own prefab's root entry:
        // resolve materializes the prefab root's components (and name) onto
        // the instance, so the inverse strips whatever still matches that
        // baseline. Without this, materialized values read as authored
        // overrides on the next resolve and prefab root edits never
        // propagate to existing instances.
        if ast.find_patch_by_type_path(node, ISA_TYPE).is_some() {
            sparsify_instance_root(ast, node, get_prefab);
            continue;
        }
        let Some(peid) = read_prefab_entity_id(ast, node) else {
            continue;
        };

        // Walk the parent chain looking for the instance root's source.
        let mut cursor = ast.ast_parent_of(node);
        let mut isa_source: Option<PathBuf> = None;
        while let Some(current) = cursor {
            if let Some(source) = read_isa_source(ast, current) {
                isa_source = Some(source);
                break;
            }
            cursor = ast.ast_parent_of(current);
        }
        let Some(source) = isa_source else {
            continue;
        };
        let Some(prefab) = get_prefab(&source) else {
            continue;
        };
        let Some(prefab_match) =
            prefab.find_node_by_component_int(PREFAB_ENTITY_ID_TYPE, u64::from(peid))
        else {
            continue;
        };

        sparsify_node_against_baseline(ast, node, prefab, prefab_match, &[PREFAB_ENTITY_ID_TYPE]);
    }
}

/// Reduce one node's components to sparse deltas against a prefab baseline
/// entry. Components equal to the baseline are removed; diverged components
/// keep only the fields that differ ([`shallow_diff`]); components absent
/// from the baseline stay whole. A name matching the baseline's name is
/// stripped too.
fn sparsify_node_against_baseline(
    ast: &mut SceneBsnAst,
    node: Entity,
    prefab: &SceneBsnAst,
    prefab_match: Entity,
    keep: &[&str],
) {
    enum Action {
        Remove,
        Replace(BsnValue),
    }
    let changes: Vec<(String, Action)> = ast
        .component_type_paths(node)
        .into_iter()
        .filter(|tp| !keep.contains(&tp.as_str()))
        .filter_map(|tp| {
            let value = get_bsn_field(ast, node, &tp, "")?;
            let base = get_bsn_field(prefab, prefab_match, &tp, "")?;
            match shallow_diff(&base, &value) {
                None => Some((tp, Action::Remove)),
                Some(delta) => Some((tp, Action::Replace(delta))),
            }
        })
        .collect();
    for (type_path, action) in changes {
        match action {
            Action::Remove => ast.remove_component_patch(node, &type_path),
            Action::Replace(delta) => set_whole_component(ast, node, &type_path, delta),
        }
    }

    // The name is a reference patch, not a component patch, so the loop
    // above never sees it; strip it too when it matches the baseline.
    if let (Some(name), Some(prefab_name)) = (ast.get_name(node), prefab.get_name(prefab_match))
        && name == prefab_name
    {
        remove_name_patch(ast, node);
    }
}

/// Strip an instance root's components (and inherited name) that still match
/// its prefab root's baseline. `IsA` and `PrefabEntityId` are kept
/// unconditionally.
fn sparsify_instance_root(ast: &mut SceneBsnAst, node: Entity, get_prefab: &PrefabLookup) {
    let Some(source) = read_isa_source(ast, node) else {
        return;
    };
    let Some(prefab) = get_prefab(&source) else {
        return;
    };
    let Some(prefab_root) = prefab
        .roots
        .iter()
        .copied()
        .find(|root| prefab.find_patch_by_type_path(*root, PREFAB_TYPE).is_some())
    else {
        return;
    };

    sparsify_node_against_baseline(
        ast,
        node,
        prefab,
        prefab_root,
        &[ISA_TYPE, PREFAB_ENTITY_ID_TYPE],
    );
}

/// Remove the name-reference patch from `node`, if it has one.
fn remove_name_patch(ast: &mut SceneBsnAst, node: Entity) {
    let Some(patches) = ast.get_patches(node) else {
        return;
    };
    let Some(pos) = patches
        .0
        .iter()
        .position(|&pe| matches!(ast.get_patch(pe), Some(BsnPatch::Name(_))))
    else {
        return;
    };
    let patch_entity = patches.0[pos];
    if let Some(patches) = ast.get_patches_mut(node) {
        patches.0.remove(pos);
    }
    ast.world.despawn(patch_entity);
}

/// The `PrefabEntityId` on `node`, read from its whole-component value. The
/// id serializes as a tuple struct wrapping one integer; a bare integer is
/// also accepted.
pub(crate) fn read_prefab_entity_id(ast: &SceneBsnAst, node: Entity) -> Option<u32> {
    let whole = get_bsn_field(ast, node, PREFAB_ENTITY_ID_TYPE, "")?;
    bsn_value_as_int(&whole).and_then(|i| u32::try_from(i).ok())
}

/// The `IsA.source` path on `isa_node`, read as a string subfield.
pub(crate) fn read_isa_source(ast: &SceneBsnAst, isa_node: Entity) -> Option<PathBuf> {
    match get_bsn_field(ast, isa_node, ISA_TYPE, "source")? {
        BsnValue::String(s) => Some(PathBuf::from(s)),
        _ => None,
    }
}

/// The `IsA.deleted` list on `isa_node`, read as a list of integer ids.
/// An absent or non-list value yields an empty vector.
pub(crate) fn read_isa_deleted(ast: &SceneBsnAst, isa_node: Entity) -> Vec<u32> {
    match get_bsn_field(ast, isa_node, ISA_TYPE, "deleted") {
        Some(BsnValue::List(items)) => items
            .iter()
            .filter_map(|item| bsn_value_as_int(item).and_then(|i| u32::try_from(i).ok()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Convert a whole-component `BsnValue` into the patch that stores it.
/// Non-component shapes (scalars, lists, maps) yield `None`.
pub(crate) fn value_to_patch(value: BsnValue) -> Option<BsnPatch> {
    match value {
        BsnValue::Struct(data) => Some(BsnPatch::Struct(data)),
        BsnValue::TupleStruct(data) => Some(BsnPatch::TupleStruct(data)),
        BsnValue::Type(tp) => Some(BsnPatch::Type(tp)),
        _ => None,
    }
}

/// Write `value` as the whole `type_path` component on `node`, replacing an
/// existing patch or inserting a new one. Reflection-registry independent:
/// it manipulates patches directly rather than routing through
/// `set_bsn_field`, so it works for any authored type path.
pub(crate) fn set_whole_component(
    ast: &mut SceneBsnAst,
    node: Entity,
    type_path: &str,
    value: BsnValue,
) {
    let Some(patch) = value_to_patch(value) else {
        return;
    };
    if let Some(patch_entity) = ast.find_patch_by_type_path(node, type_path) {
        ast.set_patch(patch_entity, patch);
        return;
    }
    let patch_entity = ast.world.spawn(patch).id();
    if let Some(patches) = ast.get_patches_mut(node) {
        patches.0.push(patch_entity);
    }
}

/// Deep-copy an entire scene document into a fresh one. Thin alias over
/// [`SceneBsnAst::deep_clone`] kept for the resolver's call sites.
pub(crate) fn clone_scene(src: &SceneBsnAst) -> SceneBsnAst {
    src.deep_clone()
}
#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_bsn::{BsnField, BsnStructData, BsnStructFields, BsnTupleStructData};

    fn f(name: &str, value: BsnValue) -> BsnField {
        BsnField {
            name: name.to_string(),
            value,
        }
    }

    fn strukt(type_path: &str, fields: Vec<BsnField>) -> BsnValue {
        BsnValue::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields(fields),
        })
    }

    fn struct_patch(type_path: &str, fields: Vec<BsnField>) -> BsnPatch {
        BsnPatch::Struct(BsnStructData {
            type_path: type_path.to_string(),
            fields: BsnStructFields(fields),
        })
    }

    fn peid_patch(id: i128) -> BsnPatch {
        BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: PREFAB_ENTITY_ID_TYPE.to_string(),
            values: vec![BsnValue::Int(id)],
        })
    }

    fn isa_patch(source: &str, deleted: Vec<i128>) -> BsnPatch {
        BsnPatch::Struct(BsnStructData {
            type_path: ISA_TYPE.to_string(),
            fields: BsnStructFields(vec![
                f("source", BsnValue::String(source.to_string())),
                f(
                    "deleted",
                    BsnValue::List(deleted.into_iter().map(BsnValue::Int).collect()),
                ),
            ]),
        })
    }

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const MESH: &str = "test::Mesh";

    fn transform(x: f64, y: f64, z: f64) -> BsnValue {
        strukt(
            TRANSFORM,
            vec![f(
                "translation",
                strukt(
                    "glam::Vec3",
                    vec![
                        f("x", BsnValue::Float(x)),
                        f("y", BsnValue::Float(y)),
                        f("z", BsnValue::Float(z)),
                    ],
                ),
            )],
        )
    }

    fn transform_patch(x: f64, y: f64, z: f64) -> BsnPatch {
        let BsnValue::Struct(data) = transform(x, y, z) else {
            unreachable!()
        };
        BsnPatch::Struct(data)
    }

    /// A prefab document: root [Prefab, PrefabEntityId(0), Transform] with a
    /// child [PrefabEntityId(1), Transform, Mesh].
    fn make_prefab() -> SceneBsnAst {
        let mut ast = SceneBsnAst::default();
        let root = ast.create_entity_node(vec![
            BsnPatch::Type(PREFAB_TYPE.to_string()),
            peid_patch(0),
            transform_patch(0.0, 0.0, 0.0),
        ]);
        let child = ast.create_entity_node(vec![
            peid_patch(1),
            transform_patch(0.0, 1.0, 0.0),
            BsnPatch::Type(MESH.to_string()),
        ]);
        ast.add_to_roots(root);
        ast.add_child_to_ast(root, child);
        ast
    }

    fn lookup<'a>(
        map: &'a std::collections::HashMap<PathBuf, SceneBsnAst>,
    ) -> impl Fn(&Path) -> Option<&'a SceneBsnAst> {
        move |p: &Path| map.get(p)
    }

    fn component_on(ast: &SceneBsnAst, node: Entity, type_path: &str) -> Option<BsnValue> {
        get_bsn_field(ast, node, type_path, "")
    }

    #[test]
    fn resolve_materializes_inherited_subtree() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());

        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn", vec![])]);
        scene.add_to_roots(instance);

        let get = lookup(&prefabs);
        let resolved = resolve_scene(&scene, &get).expect("resolves");

        let root = resolved.roots[0];
        // Root inheritance copied Transform (and the root's PrefabEntityId)
        // onto the instance root.
        let root_transform = component_on(&resolved, root, TRANSFORM).expect("root transform");
        assert!(bsn_value_eq(&root_transform, &transform(0.0, 0.0, 0.0)));

        // Descendant materialization produced the single inherited child.
        let kids = resolved.get_children_ast(root);
        assert_eq!(kids.len(), 1, "one inherited child");
        let child = kids[0];
        assert_eq!(read_prefab_entity_id(&resolved, child), Some(1));
        let child_transform = component_on(&resolved, child, TRANSFORM).expect("child transform");
        assert!(bsn_value_eq(&child_transform, &transform(0.0, 1.0, 0.0)));
        assert!(resolved.find_patch_by_type_path(child, MESH).is_some());
    }

    #[test]
    fn authored_override_survives_resolve() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());

        // The instance authors a sparse override on the inherited child:
        // only translation.y differs.
        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn", vec![])]);
        let override_child = scene.create_entity_node(vec![
            peid_patch(1),
            struct_patch(
                TRANSFORM,
                vec![f(
                    "translation",
                    strukt("glam::Vec3", vec![f("y", BsnValue::Float(9.0))]),
                )],
            ),
        ]);
        scene.add_to_roots(instance);
        scene.add_child_to_ast(instance, override_child);

        let get = lookup(&prefabs);
        let resolved = resolve_scene(&scene, &get).expect("resolves");

        let root = resolved.roots[0];
        let kids = resolved.get_children_ast(root);
        assert_eq!(kids.len(), 1, "override reuses the existing child");
        let child = kids[0];
        // The delta merged onto the baseline: y overridden, x/z inherited.
        let child_transform = component_on(&resolved, child, TRANSFORM).expect("child transform");
        assert!(
            bsn_value_eq(&child_transform, &transform(0.0, 9.0, 0.0)),
            "override merges onto the inherited baseline"
        );
        // The fill-in supplied the Mesh the override omitted.
        assert!(resolved.find_patch_by_type_path(child, MESH).is_some());
    }

    #[test]
    fn deleted_id_is_not_materialized() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());

        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn", vec![1])]);
        scene.add_to_roots(instance);

        let get = lookup(&prefabs);
        let resolved = resolve_scene(&scene, &get).expect("resolves");

        let root = resolved.roots[0];
        assert!(
            resolved.get_children_ast(root).is_empty(),
            "the deleted child is not materialized"
        );
    }

    #[test]
    fn nested_prefab_expands() {
        // inner.bsn: root [Prefab, PrefabEntityId(0), Transform] + child
        // [PrefabEntityId(1), Mesh].
        let mut inner = SceneBsnAst::default();
        let inner_root = inner.create_entity_node(vec![
            BsnPatch::Type(PREFAB_TYPE.to_string()),
            peid_patch(0),
            transform_patch(0.0, 0.0, 0.0),
        ]);
        let inner_child =
            inner.create_entity_node(vec![peid_patch(1), BsnPatch::Type(MESH.to_string())]);
        inner.add_to_roots(inner_root);
        inner.add_child_to_ast(inner_root, inner_child);

        // outer.bsn: root [Prefab, PrefabEntityId(0)] whose child is an
        // instance of inner.bsn carrying its own PrefabEntityId(2).
        let mut outer = SceneBsnAst::default();
        let outer_root =
            outer.create_entity_node(vec![BsnPatch::Type(PREFAB_TYPE.to_string()), peid_patch(0)]);
        let outer_instance =
            outer.create_entity_node(vec![peid_patch(2), isa_patch("inner.bsn", vec![])]);
        outer.add_to_roots(outer_root);
        outer.add_child_to_ast(outer_root, outer_instance);

        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("inner.bsn"), inner);
        prefabs.insert(PathBuf::from("outer.bsn"), outer);

        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("outer.bsn", vec![])]);
        scene.add_to_roots(instance);

        let get = lookup(&prefabs);
        let resolved = resolve_scene(&scene, &get).expect("resolves");

        let root = resolved.roots[0];
        // outer's descendant (the inner instance) materialized under the root.
        let outer_kids = resolved.get_children_ast(root);
        assert_eq!(outer_kids.len(), 1, "outer instance materialized");
        let inner_instance = outer_kids[0];
        assert_eq!(read_prefab_entity_id(&resolved, inner_instance), Some(2));
        // The nested instance was expanded before merging, so its inherited
        // child (Mesh, PrefabEntityId(1)) is present.
        let inner_kids = resolved.get_children_ast(inner_instance);
        assert_eq!(inner_kids.len(), 1, "nested inherited child present");
        assert!(
            resolved
                .find_patch_by_type_path(inner_kids[0], MESH)
                .is_some()
        );
    }

    #[test]
    fn sparsify_inverts_resolve() {
        let mut prefabs = std::collections::HashMap::new();
        prefabs.insert(PathBuf::from("prefab.bsn"), make_prefab());

        let mut scene = SceneBsnAst::default();
        let instance = scene.create_entity_node(vec![isa_patch("prefab.bsn", vec![])]);
        scene.add_to_roots(instance);

        let get = lookup(&prefabs);
        let resolved = resolve_scene(&scene, &get).expect("resolves");

        // Strip the resolved doc back to sparse form, then resolve again.
        let mut stripped = clone_scene(&resolved);
        sparsify_inherited_descendants(&mut stripped, &get);

        // The stripped inherited child kept only its PrefabEntityId (its
        // Transform and Mesh matched the baseline).
        let stripped_root = stripped.roots[0];
        let stripped_child = stripped.get_children_ast(stripped_root)[0];
        let mut remaining = stripped.component_type_paths(stripped_child);
        remaining.sort();
        assert_eq!(remaining, vec![PREFAB_ENTITY_ID_TYPE.to_string()]);

        let reresolved = resolve_scene(&stripped, &get).expect("re-resolves");
        let a_root = resolved.roots[0];
        let b_root = reresolved.roots[0];
        assert_nodes_eq(&resolved, a_root, &reresolved, b_root);
    }

    /// Assert two resolved nodes carry equal components (by value) and equal
    /// child subtrees, so `resolve(sparsify(resolved)) == resolved`.
    fn assert_nodes_eq(a: &SceneBsnAst, an: Entity, b: &SceneBsnAst, bn: Entity) {
        let mut a_types = a.component_type_paths(an);
        let mut b_types = b.component_type_paths(bn);
        a_types.sort();
        b_types.sort();
        assert_eq!(a_types, b_types, "component sets match");
        for tp in &a_types {
            let av = get_bsn_field(a, an, tp, "").expect("a value");
            let bv = get_bsn_field(b, bn, tp, "").expect("b value");
            assert!(bsn_value_eq(&av, &bv), "component {tp} matches by value");
        }
        let a_kids = a.get_children_ast(an);
        let b_kids = b.get_children_ast(bn);
        assert_eq!(a_kids.len(), b_kids.len(), "same child count");
        for (ak, bk) in a_kids.iter().zip(&b_kids) {
            assert_nodes_eq(a, *ak, b, *bk);
        }
    }
}
