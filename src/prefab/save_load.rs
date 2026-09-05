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
        let path = resolve_source_path(&source.to_string_lossy(), scene_dir);
        if cache.get(&path).is_none()
            && let Ok(prefab_ast) = read_prefab_ast(&path)
        {
            cache.insert(path.clone(), prefab_ast);
        }
        paths.push(path);
    }
    paths
}

fn resolve_source_path(source: &str, scene_dir: &Path) -> PathBuf {
    let p = Path::new(source);
    let resolved = if p.is_absolute() {
        p.to_path_buf()
    } else {
        scene_dir.join(p)
    };
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

pub(crate) fn read_prefab_ast(path: &Path) -> Result<SceneBsnAst, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    if path.extension().is_some_and(|e| e == "bsn") {
        let mut ast = jackdaw_bsn::parse_bsn_text(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        // A plain hand-authored scene has no `Prefab` marker (and may have
        // several roots); wrap it so the resolver can instance it. Real
        // prefab files are left untouched.
        let display_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
        crate::prefab::operators::normalize_as_prefab_source(&mut ast, display_name);
        return Ok(ast);
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
