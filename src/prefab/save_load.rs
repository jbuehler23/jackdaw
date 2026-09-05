//! Glue between the editor's `scene_io` and the prefab cache / resolver.

use crate::prefab::cache::PrefabAstCache;
use jackdaw_bsn::SceneBsnAst;
use std::path::{Path, PathBuf};

const ISA_TYPE: &str = "jackdaw::prefab::components::IsA";

/// Walk a freshly parsed scene document for `IsA` references and load /
/// cache each referenced prefab. Returns the list of prefab paths the
/// watcher should track.
pub fn populate_cache_for_scene_bsn(
    ast: &SceneBsnAst,
    cache: &mut PrefabAstCache,
    scene_dir: &Path,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = crate::prefab::resolver_bsn::read_isa_source(ast, node) else {
            continue;
        };
        let path = resolve_source_path(&source, scene_dir);
        cache_prefab_tree(&path, cache);
        paths.push(path);
    }
    paths
}

/// Cache `path` and every prefab it transitively inherits from. The resolver
/// expands a whole `IsA` chain in one pass, so a partly cached chain fails.
pub(crate) fn cache_prefab_tree(path: &Path, cache: &mut PrefabAstCache) {
    cache_prefab_tree_inner(path, cache, 0);
}

fn cache_prefab_tree_inner(path: &Path, cache: &mut PrefabAstCache, depth: usize) {
    // The same bound the resolver enforces; it also terminates an `IsA` cycle,
    // since an already-cached document is still walked.
    if depth >= crate::prefab::resolver_bsn::MAX_PREFAB_DEPTH {
        return;
    }
    if cache.get(path).is_none() {
        match read_prefab_ast(path) {
            Ok(prefab_ast) => cache.insert(path, prefab_ast),
            Err(_) => return,
        }
    }
    // `read_prefab_document` absolutizes each `IsA` source against the prefab's
    // own directory.
    let Some(prefab_ast) = cache.get(path) else {
        return;
    };
    let nested: Vec<PathBuf> = prefab_ast
        .entities_with_component(ISA_TYPE)
        .into_iter()
        .filter_map(|node| crate::prefab::resolver_bsn::read_isa_source(prefab_ast, node))
        .collect();
    let prefab_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    for source in nested {
        let nested_path = resolve_source_path(&source, &prefab_dir);
        cache_prefab_tree_inner(&nested_path, cache, depth + 1);
    }
}

/// Point every `IsA` source at the file the editor is going to read, so the
/// document, the cache, the resolver and the next save all name the same file.
pub fn retarget_isa_sources(ast: &mut SceneBsnAst, scene_dir: &Path) {
    for node in ast.entities_with_component(ISA_TYPE) {
        let Some(source) = jackdaw_prefab::read_isa_source(ast, node) else {
            continue;
        };
        let resolved = resolve_source_path(&source, scene_dir);
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
pub(crate) fn resolve_source_path(source: &Path, scene_dir: &Path) -> PathBuf {
    let resolved = jackdaw_prefab::source_path(source, scene_dir);
    // Scenes written before (or after) their prefab converted formats may
    // reference the other extension; fall back to the sibling.
    if !resolved.exists() {
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
pub fn read_prefab_ast(path: &Path) -> Result<SceneBsnAst, std::io::Error> {
    if path.extension().is_some_and(|e| e == "bsn") {
        return jackdaw_prefab::source::read_prefab_document(path);
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
