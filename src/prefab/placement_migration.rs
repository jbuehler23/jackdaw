//! Moving the transform a packed prefab's lone node carries onto its
//! instances.
//!
//! Packing once left a lone root's rotation and scale on the node inside the
//! prefab and gave each instance only what remained, so an instance could not
//! stand at a scale that differs by axis. The operator here rewrites such a
//! prefab with its node at identity, and every document instancing it with
//! the whole placement on the instance, so the scene looks as it did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::report_to_caller;
use jackdaw_bsn::{BsnValue, SceneBsnAst, get_bsn_field};
use path_slash::PathExt as _;

use crate::prefab::resolver_bsn::{read_isa_source, read_prefab_entity_id, set_whole_component};

const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";
const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";
const TRANSFORM_TYPE: &str = "bevy_transform::components::transform::Transform";

/// How far a folded placement may stray from the matrix it stands for.
const FOLD_TOLERANCE: f32 = 1e-4;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PrefabMigratePackedTransformsOp>();
}

/// Move every packed prefab's inner transform onto its instances.
#[operator(
    id = "prefab.migrate_packed_transforms",
    label = "Migrate Packed Prefab Transforms",
    description = "Stand the lone node of every packed prefab at identity and give each instance \
                   of it the whole placement instead, so instances can carry any scale. Writes \
                   over the project's files and cannot be undone; save what is open first.",
    allows_undo = false
)]
pub fn prefab_migrate_packed_transforms(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(migrate);
    OperatorResult::Finished
}

/// One document under the assets folder, as read and as it will be written.
struct Document {
    file: PathBuf,
    preamble: Option<String>,
    ast: SceneBsnAst,
    changed: bool,
}

/// A prefab whose lone node carries a transform, the placement each instance
/// of it takes on, and why the fold is off when one cannot.
struct Candidate {
    document: usize,
    node: Entity,
    node_id: u32,
    inner: Transform,
    root: Transform,
    instances: Vec<(usize, Entity, Transform)>,
    blocked: Option<String>,
}

fn migrate(world: &mut World) {
    let op = "prefab.migrate_packed_transforms";
    let Some(assets) = crate::asset_index::assets_dir(world).filter(|dir| dir.is_dir()) else {
        report_to_caller(world, format!("{op}: no project is open"));
        return;
    };
    if let Some(reason) = crate::asset_migration::open_edit(world) {
        report_to_caller(world, format!("{op}: refused, {reason}"));
        return;
    }

    let mut documents = read_documents(&assets);
    let mut candidates = find_candidates(&documents);
    collect_instances(world, &documents, &mut candidates);

    let mut folded: Vec<String> = Vec::new();
    let mut left: Vec<String> = Vec::new();
    let mut paths: Vec<&PathBuf> = candidates.keys().collect();
    paths.sort();
    for path in paths {
        let candidate = &candidates[path];
        let name = relative_name(&assets, path);
        if let Some(reason) = &candidate.blocked {
            left.push(format!("{name} ({reason})"));
            continue;
        }
        let prefab = &mut documents[candidate.document];
        set_whole_component(
            &mut prefab.ast,
            candidate.node,
            TRANSFORM_TYPE,
            super::operators::transform_value(Transform::IDENTITY),
        );
        prefab.changed = true;
        for &(document, node, placement) in &candidate.instances {
            let document = &mut documents[document];
            set_whole_component(
                &mut document.ast,
                node,
                TRANSFORM_TYPE,
                super::operators::transform_value(placement),
            );
            document.changed = true;
        }
        let count = candidate.instances.len();
        let noun = if count == 1 { "instance" } else { "instances" };
        folded.push(format!("{name} ({count} {noun})"));
    }

    let written = write_changed(world, &documents, &mut left, &assets);
    refresh_open_documents(world, &written, &candidates);

    if folded.is_empty() && left.is_empty() {
        report_to_caller(world, format!("{op}: nothing to migrate"));
        return;
    }
    if !folded.is_empty() {
        report_to_caller(world, format!("{op}: folded {}", folded.join(", ")));
    }
    if !left.is_empty() {
        report_to_caller(world, format!("{op}: left alone {}", left.join(", ")));
    }
    if !written.is_empty() {
        report_to_caller(world, format!("{op}: undo does not reach these files"));
    }
}

/// Every document under `assets` that reads.
fn read_documents(assets: &Path) -> Vec<Document> {
    jackdaw_bsn::walk_document_files(assets)
        .into_iter()
        .filter(|file| jackdaw_bsn::is_document_path(file))
        .filter_map(|file| {
            let text = jackdaw_bsn::read_document_text(&file).ok()?;
            let ast = jackdaw_bsn::parse_bsn_text(&text).ok()?;
            Some(Document {
                preamble: jackdaw_bsn::leading_comments(&text),
                file: dunce::canonicalize(&file).unwrap_or(file),
                ast,
                changed: false,
            })
        })
        .collect()
}

/// The prefabs whose root holds a single node that turns or scales about the
/// root's origin, as packing a lone root left it, keyed by the file each is.
fn find_candidates(documents: &[Document]) -> HashMap<PathBuf, Candidate> {
    let mut candidates = HashMap::new();
    for (index, document) in documents.iter().enumerate() {
        let ast = &document.ast;
        let [root] = ast.roots.as_slice() else {
            continue;
        };
        if ast.find_patch_by_type_path(*root, PREFAB_TYPE).is_none()
            || ast.find_patch_by_type_path(*root, ISA_TYPE).is_some()
        {
            continue;
        }
        let children = ast.get_children_ast(*root);
        let [node] = children.as_slice() else {
            continue;
        };
        let Some(inner) = read_transform(ast, *node, Transform::IDENTITY) else {
            continue;
        };
        let Some(node_id) = read_prefab_entity_id(ast, *node) else {
            continue;
        };
        if is_identity(&inner) || !inner.translation.abs_diff_eq(Vec3::ZERO, FOLD_TOLERANCE) {
            continue;
        }
        candidates.insert(
            document.file.clone(),
            Candidate {
                document: index,
                node: *node,
                node_id,
                inner,
                root: read_transform(ast, *root, Transform::IDENTITY)
                    .unwrap_or(Transform::IDENTITY),
                instances: Vec::new(),
                blocked: None,
            },
        );
    }
    candidates
}

/// Find every instance of every candidate, blocking a candidate one of whose
/// instances cannot take its transform over.
fn collect_instances(
    world: &World,
    documents: &[Document],
    candidates: &mut HashMap<PathBuf, Candidate>,
) {
    for (index, document) in documents.iter().enumerate() {
        let ast = &document.ast;
        let document_dir = document.file.parent().unwrap_or(Path::new("."));
        let assets_root = crate::prefab::save_load::source_root(world, document_dir);
        for node in ast_nodes(ast) {
            let Some(source) = read_isa_source(ast, node) else {
                continue;
            };
            let source =
                crate::prefab::save_load::resolve_source_path(&source, &assets_root, document_dir);
            let source = dunce::canonicalize(&source).unwrap_or(source);
            let Some(candidate) = candidates.get_mut(&source) else {
                continue;
            };
            if candidate.blocked.is_some() {
                continue;
            }
            let file = document.file.display();
            if ast.roots.contains(&node) && ast.find_patch_by_type_path(node, PREFAB_TYPE).is_some()
            {
                candidate.blocked = Some(format!("{file} is a variant of it"));
                continue;
            }
            if overrides_node_transform(ast, node, candidate.node_id) {
                candidate.blocked = Some(format!("an instance in {file} moves its node"));
                continue;
            }
            let Some(placement) = read_transform(ast, node, candidate.root) else {
                candidate.blocked =
                    Some(format!("an instance in {file} has an unreadable transform"));
                continue;
            };
            let Some(folded) = fold(placement, candidate.inner) else {
                candidate.blocked = Some(format!(
                    "an instance in {file} is scaled unevenly across its turned node"
                ));
                continue;
            };
            candidate.instances.push((index, node, folded));
        }
    }
}

/// Every node of a document, roots first and each subtree in order.
fn ast_nodes(ast: &SceneBsnAst) -> Vec<Entity> {
    let mut nodes = Vec::new();
    let mut stack: Vec<Entity> = ast.roots.iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        nodes.push(node);
        stack.extend(ast.get_children_ast(node).into_iter().rev());
    }
    nodes
}

/// Whether the instance at `instance` overrides the transform of the prefab
/// node numbered `node_id`.
fn overrides_node_transform(ast: &SceneBsnAst, instance: Entity, node_id: u32) -> bool {
    ast.get_children_ast(instance).into_iter().any(|child| {
        read_prefab_entity_id(ast, child) == Some(node_id)
            && ast.find_patch_by_type_path(child, TRANSFORM_TYPE).is_some()
    })
}

/// `outer` then `inner` as one transform, or `None` when the two shear and no
/// rotation and scale can stand for them.
fn fold(outer: Transform, inner: Transform) -> Option<Transform> {
    let composed = outer.to_matrix() * inner.to_matrix();
    let folded = Transform::from_matrix(composed);
    let span = composed
        .abs()
        .to_cols_array()
        .into_iter()
        .fold(1.0_f32, f32::max);
    composed
        .abs_diff_eq(folded.to_matrix(), FOLD_TOLERANCE * span)
        .then_some(folded)
}

fn is_identity(transform: &Transform) -> bool {
    transform
        .translation
        .abs_diff_eq(Vec3::ZERO, FOLD_TOLERANCE)
        && transform
            .rotation
            .abs_diff_eq(Quat::IDENTITY, FOLD_TOLERANCE)
        && transform.scale.abs_diff_eq(Vec3::ONE, FOLD_TOLERANCE)
}

/// The transform a node's `Transform` patch spells, its missing fields taken
/// from `base`. `None` when a field it does spell does not read.
fn read_transform(ast: &SceneBsnAst, node: Entity, base: Transform) -> Option<Transform> {
    let Some(value) = get_bsn_field(ast, node, TRANSFORM_TYPE, "") else {
        return Some(base);
    };
    let BsnValue::Struct(data) = value else {
        return Some(base);
    };
    let mut transform = base;
    for field in &data.fields.0 {
        match field.name.as_str() {
            "translation" => transform.translation = read_vec3(&field.value, base.translation)?,
            "scale" => transform.scale = read_vec3(&field.value, base.scale)?,
            "rotation" => transform.rotation = read_quat(&field.value, base.rotation)?,
            _ => {}
        }
    }
    Some(transform)
}

fn read_axes<const N: usize>(
    value: &BsnValue,
    names: [&str; N],
    base: [f32; N],
) -> Option<[f32; N]> {
    let BsnValue::Struct(data) = value else {
        return None;
    };
    let mut out = base;
    for field in &data.fields.0 {
        let Some(slot) = names.iter().position(|name| *name == field.name) else {
            continue;
        };
        out[slot] = match field.value {
            BsnValue::Float(v) => v as f32,
            BsnValue::Int(v) => v as f32,
            _ => return None,
        };
    }
    Some(out)
}

fn read_vec3(value: &BsnValue, base: Vec3) -> Option<Vec3> {
    read_axes(value, ["x", "y", "z"], base.to_array()).map(Vec3::from_array)
}

fn read_quat(value: &BsnValue, base: Quat) -> Option<Quat> {
    read_axes(value, ["x", "y", "z", "w"], base.to_array()).map(Quat::from_array)
}

/// Write every changed document back in the form it was read in, returning
/// the files written.
fn write_changed(
    world: &mut World,
    documents: &[Document],
    left: &mut Vec<String>,
    assets: &Path,
) -> Vec<PathBuf> {
    let mut written = Vec::new();
    for document in documents.iter().filter(|document| document.changed) {
        let text = jackdaw_bsn::document_as_text(&document.ast, document.preamble.as_deref());
        let result = jackdaw_bsn::document_bytes(&document.file, &text)
            .map_err(|err| err.to_string())
            .and_then(|bytes| {
                crate::scene_io::save::write_atomic(&document.file, &bytes)
                    .map_err(|err| err.to_string())
            });
        match result {
            Ok(()) => {
                crate::asset_migration::note_written(world, &document.file);
                written.push(document.file.clone());
            }
            Err(err) => left.push(format!("{} ({err})", relative_name(assets, &document.file))),
        }
    }
    written
}

/// Read the rewritten prefabs and the open scenes again, so what is on screen
/// is what is now on disk.
fn refresh_open_documents(
    world: &mut World,
    written: &[PathBuf],
    candidates: &HashMap<PathBuf, Candidate>,
) {
    if written.is_empty() {
        return;
    }
    for path in candidates.keys().filter(|path| written.contains(path)) {
        let assets_root = crate::prefab::save_load::source_root_of(world, path);
        let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
        cache.invalidate(path);
        crate::prefab::save_load::cache_prefab_tree(path, &mut cache, &assets_root);
    }
    let open: Vec<PathBuf> = world
        .resource::<crate::scenes::Scenes>()
        .tabs
        .iter()
        .filter_map(|tab| tab.path.as_ref())
        .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .filter(|path| written.contains(path))
        .collect();
    for path in &open {
        crate::scenes::external_watch::reread_tab_from_disk(world, path);
    }
    if open.is_empty() {
        crate::prefab::watcher::reload_all_instances(world);
    }
}

fn relative_name(assets: &Path, path: &Path) -> String {
    let assets = dunce::canonicalize(assets).unwrap_or_else(|_| assets.to_path_buf());
    path.strip_prefix(&assets)
        .unwrap_or(path)
        .to_slash_lossy()
        .into_owned()
}
