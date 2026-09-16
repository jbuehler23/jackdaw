//! Glue between the editor's `scene_io` and the prefab cache / resolver.

use bevy::prelude::{World, warn};

use crate::prefab::cache::PrefabAstCache;
use jackdaw_bsn::SceneBsnAst;
use std::path::{Path, PathBuf};

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

/// The open project's assets folder, the one namespace its documents name
/// files in.
///
/// Canonical, since a path reached through a symlink is the same file as the
/// path the project names and a lexical comparison cannot see that.
fn assets_namespace(world: &World) -> Option<PathBuf> {
    assets_folder(world).map(|assets| dunce::canonicalize(&assets).unwrap_or(assets))
}

/// The assets folder as the project spells it, before any symlink is followed.
fn assets_folder(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
        .filter(|assets| assets.is_dir())
}

/// The folder every prefab source is spelled relative to: the open project's
/// assets folder, or `document_dir` when no project is open.
pub fn source_root(world: &World, document_dir: &Path) -> PathBuf {
    assets_namespace(world).unwrap_or_else(|| document_dir.to_path_buf())
}

/// Spell every prefab source in the copy about to be written the way the
/// project names files, reporting the ones it cannot.
///
/// With a project open that is the path under its assets folder, and a source
/// from outside it names a place on one machine. A document standing on its
/// own has no such folder, so its sources are written relative to itself.
pub fn relativize_for_file(
    world: &World,
    ast: &mut jackdaw_bsn::SceneBsnAst,
    document_dir: &Path,
) -> Vec<String> {
    let Some(assets) = assets_namespace(world) else {
        jackdaw_prefab::relativize_isa_sources(ast, document_dir);
        return Vec::new();
    };
    let strays = jackdaw_prefab::relativize_isa_sources_under(ast, &assets);
    match assets_folder(world).filter(|folder| !strays.is_empty() && folder != &assets) {
        Some(folder) => jackdaw_prefab::relativize_isa_sources_under(ast, &folder),
        None => strays,
    }
}

/// [`source_root`] for the document held at `path`.
pub fn source_root_of(world: &World, path: &Path) -> PathBuf {
    source_root(world, path.parent().unwrap_or(Path::new(".")))
}

/// Walk a freshly parsed scene document for `IsA` references and load /
/// cache each referenced prefab. Returns the list of prefab paths the
/// watcher should track.
pub fn populate_cache_for_scene_bsn(
    ast: &SceneBsnAst,
    cache: &mut PrefabAstCache,
    assets_root: &Path,
    document_dir: &Path,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = crate::prefab::resolver_bsn::read_isa_source(ast, node) else {
            continue;
        };
        let path = resolve_source_path(&source, assets_root, document_dir);
        cache_prefab_tree(&path, cache, assets_root);
        paths.push(path);
    }
    paths
}

/// Warn for every instance in `ast` whose prefab the cache has not got, naming
/// the file each one points at.
pub fn warn_for_missing_sources(ast: &SceneBsnAst, cache: &PrefabAstCache, scene: &str) {
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = crate::prefab::resolver_bsn::read_isa_source(ast, node) else {
            continue;
        };
        if cache.get(&source).is_none() {
            warn!(
                "scene '{scene}': the prefab '{}' {}, so the instance naming it inherits nothing",
                source.display(),
                missing_source_reason(&source)
            );
        }
    }
}

/// Why an instance inherited nothing: the file its source names is not there,
/// or it is there and did not read as a document.
pub fn missing_source_reason(path: &Path) -> &'static str {
    match path.exists() {
        true => "could not be read",
        false => "is not in the project",
    }
}

/// Cache `path` and every prefab it transitively inherits from. The resolver
/// expands a whole `IsA` chain in one pass, so a partly cached chain fails.
pub(crate) fn cache_prefab_tree(path: &Path, cache: &mut PrefabAstCache, assets_root: &Path) {
    cache_prefab_tree_inner(path, cache, assets_root, 0);
}

fn cache_prefab_tree_inner(
    path: &Path,
    cache: &mut PrefabAstCache,
    assets_root: &Path,
    depth: usize,
) {
    // The same bound the resolver enforces; it also terminates an `IsA` cycle,
    // since an already-cached document is still walked.
    if depth >= crate::prefab::resolver_bsn::MAX_PREFAB_DEPTH {
        return;
    }
    if cache.get(path).is_none() {
        match read_prefab_ast(path, assets_root) {
            Ok(prefab_ast) => {
                cache.insert(path, prefab_ast);
                // The file as read is the watcher's baseline: the first look
                // it takes when the watch starts must not read as an edit.
                if let Ok(fingerprint) = crate::prefab::cache::compute_file_fingerprint(path) {
                    cache.record_saved_fingerprint(path, fingerprint);
                }
            }
            Err(_) => return,
        }
    }
    let Some(prefab_ast) = cache.get(path) else {
        return;
    };
    let nested: Vec<PathBuf> = prefab_ast
        .entities_with_component(ISA_TYPE)
        .into_iter()
        .filter_map(|node| crate::prefab::resolver_bsn::read_isa_source(prefab_ast, node))
        .collect();
    let nested_dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    for source in nested {
        let nested_path = resolve_source_path(&source, assets_root, &nested_dir);
        cache_prefab_tree_inner(&nested_path, cache, assets_root, depth + 1);
    }
}

/// Point every `IsA` source at the file the editor is going to read, so the
/// document, the cache, the resolver and the next save all name the same file.
pub fn retarget_isa_sources(ast: &mut SceneBsnAst, assets_root: &Path, document_dir: &Path) {
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = jackdaw_prefab::read_isa_source(ast, node) else {
            continue;
        };
        let resolved = resolve_source_path(&source, assets_root, document_dir);
        if resolved == source {
            continue;
        }
        let deleted = jackdaw_prefab::read_isa_deleted(ast, node);
        jackdaw_prefab::set_whole_component(
            ast,
            node,
            ISA_TYPE,
            jackdaw_prefab::isa_value(&resolved.to_string_lossy(), &deleted),
        );
    }
}

/// The prefab file an instance's `source` names, with a fallback for a scene
/// naming the other scene format.
pub(crate) fn resolve_source_path(
    source: &Path,
    assets_root: &Path,
    document_dir: &Path,
) -> PathBuf {
    let resolved = jackdaw_prefab::source_path(source, assets_root, document_dir);
    // Scenes written before (or after) their prefab converted formats may
    // reference the other extension; fall back to the sibling.
    if !resolved.exists() {
        if let Some(held) = jackdaw_bsn::existing_form(&resolved) {
            return held;
        }
        let sibling = match resolved.extension().and_then(|e| e.to_str()) {
            Some("jsn") => Some(resolved.with_extension("bsn")),
            Some("bsn") => Some(resolved.with_extension("jsn")),
            _ => None,
        };
        if let Some(sibling) = sibling
            && sibling.exists()
        {
            return sibling;
        }
    }
    resolved
}

/// Read a prefab file into a document.
pub fn read_prefab_ast(path: &Path, assets_root: &Path) -> Result<SceneBsnAst, std::io::Error> {
    if jackdaw_bsn::is_document_path(path) {
        return jackdaw_prefab::source::read_prefab_document(path, assets_root);
    }
    // TODO: legacy `.jsn` prefabs with no `.bsn` sibling cannot be cached
    // here. A faithful `.jsn` -> BSN document conversion needs a `World` and
    // its type registry to recover the type paths of nested component values,
    // which the worldless structural bridge drops. The editor writes prefabs
    // as `.bsn` and `resolve_source_path` prefers a `.bsn` sibling, so this
    // only affects genuinely legacy files; callers tolerate the error by
    // skipping the entry.
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "legacy .jsn prefab {} cannot be cached as a BSN document without a world",
            path.display()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_bsn::{BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnValue};

    fn scene_with_source(source: &Path) -> (SceneBsnAst, bevy::prelude::Entity) {
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(vec![BsnPatch::Struct(BsnStructData {
            type_path: ISA_TYPE.to_string(),
            fields: BsnStructFields(vec![BsnField {
                name: "source".to_string(),
                value: BsnValue::String(source.to_string_lossy().into_owned()),
            }]),
        })]);
        ast.add_to_roots(node);
        (ast, node)
    }

    #[test]
    #[cfg(unix)]
    fn a_project_opened_through_a_symlink_still_spells_its_sources_under_its_assets_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("assets/prefabs")).expect("the folders are made");
        let prefab = real.join("assets/prefabs/lamp.bsn");
        std::fs::write(&prefab, "#Lamp\n").expect("the prefab is written");
        let through_a_link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &through_a_link).expect("the link is made");

        let mut world = World::new();
        world.insert_resource(crate::project::ProjectRoot::new(
            &through_a_link,
            Default::default(),
        ));
        let canonical = dunce::canonicalize(&prefab).expect("the prefab is on disk");
        let (mut ast, node) = scene_with_source(&canonical);

        let strays = relativize_for_file(&world, &mut ast, &real.join("assets"));

        assert!(strays.is_empty(), "the file is under the assets folder");
        assert_eq!(
            jackdaw_prefab::read_isa_source(&ast, node).expect("the source stands"),
            PathBuf::from("prefabs/lamp.bsn"),
        );
    }
}
