//! Convert a legacy `JsnScene` into equivalent `.bsn` scene text and an asset
//! catalog.
//!
//! The conversion round-trips through ECS and reflection rather than mapping
//! JSON structurally. A `JsnScene` is spawned into the world with the editor's
//! Handle-aware deserialization (so material and texture handles resolve to real
//! assets), each live entity's components are reflected into plain BSN patches,
//! and the resulting document is emitted as `.bsn` text. Inline assets convert
//! into a separate catalog document.
//!
//! Emitted components are always plain: no `@template` or `:base` inheritance.
//! The stable node id travels as an ordinary `SceneNodeId` component patch.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use bevy::asset::UntypedHandle;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::prelude::*;

use jackdaw_bsn::{
    BsnAssetContext, CatalogAssetRef, SceneBsnAst, component_to_bsn_patch_with_assets,
    create_entity_in_ast, emit_scene, serialize_assets_to_bsn,
};
use jackdaw_jsn::JsnScene;

use crate::scene_io::{load_inline_assets, load_scene_from_jsn, should_skip_component};

/// The result of converting a `JsnScene`: the scene `.bsn` text (with any
/// scene-inline assets embedded as named asset roots) and a short report of
/// what was converted.
pub struct ConvertedScene {
    /// Scene entities, components, and inline assets as `.bsn` text.
    pub scene_bsn: String,
    /// Summary of the conversion for logging and tooling.
    pub report: ConversionReport,
}

/// Counts and names describing what a conversion produced.
pub struct ConversionReport {
    /// Number of scene entities converted.
    pub entity_count: usize,
    /// Number of named assets written to the catalog.
    pub asset_count: usize,
}

/// Convert an already-parsed `JsnScene` into `.bsn` scene text plus an asset
/// catalog.
///
/// `world` supplies the editor's populated `TypeRegistry`, `AssetServer`, and
/// `Assets<T>` stores. Entities are spawned transiently to reflect their
/// components and are despawned again before returning, and the live
/// `SceneBsnAst` document (if any) is preserved.
pub fn convert_jsn_scene_to_bsn(
    world: &mut World,
    scene: &JsnScene,
) -> Result<ConvertedScene, BevyError> {
    convert_jsn_scene_to_bsn_at(world, scene, Path::new(""))
}

/// Parse `.jsn` text (v3, falling back to the v2 layout) and convert it. Mirrors
/// the loader's parse boundary so migration tooling can accept raw file bytes.
pub fn convert_jsn_text(world: &mut World, text: &str) -> Result<ConvertedScene, BevyError> {
    let (scene, _version) = jackdaw_jsn::format::parse_scene(text)
        .map_err(|e| BevyError::from(format!("could not parse .jsn: {e}")))?;
    convert_jsn_scene_to_bsn(world, &scene)
}

/// `convert_jsn_scene_to_bsn` with an explicit `parent_path`, the directory the
/// emitted `.bsn` would live in. Asset paths are emitted relative to it.
pub fn convert_jsn_scene_to_bsn_at(
    world: &mut World,
    scene: &JsnScene,
    parent_path: &Path,
) -> Result<ConvertedScene, BevyError> {
    // Files written before the scene types moved crates carry old component
    // type paths; rewrite them so registry lookups resolve.
    let scene = {
        let mut scene = scene.clone();
        jackdaw_jsn::format::canonicalize_scene(&mut scene);
        scene
    };
    let scene = &scene;

    // Set the fresh AST aside the editor's live document first, so nothing the
    // transient spawn does can touch it. Restore whatever was there afterward.
    let saved_ast = world.remove_resource::<SceneBsnAst>();
    world.insert_resource(SceneBsnAst::default());

    // Resolve inline assets first so component handles can bind to them, then
    // spawn the scene with the Handle-aware loader (materials/textures resolve
    // to real handles instead of the null placeholder JSON stores for them).
    let local_assets = load_inline_assets(world, &scene.assets, parent_path);
    let spawned = load_scene_from_jsn(world, &scene.scene, parent_path, &local_assets);

    // Inline assets have no filesystem path; emit their reference names.
    let asset_names: bevy::platform::collections::HashMap<bevy::asset::UntypedAssetId, String> =
        local_assets
            .iter()
            .map(|(name, handle)| (handle.id(), name.clone()))
            .collect();

    // Scene-inline assets embed as named asset roots in the same document,
    // ahead of the entity roots, mirroring how a catalog file names entries.
    let asset_refs = catalog_refs(&local_assets);
    let asset_count = asset_refs.len();
    let scene_bsn = build_scene_bsn(world, &spawned, parent_path, &asset_names, &asset_refs);

    world.remove_resource::<SceneBsnAst>();
    if let Some(ast) = saved_ast {
        world.insert_resource(ast);
    }

    let entity_count = spawned.len();

    // Despawn the transient entities so the conversion leaves no residue.
    for &entity in &spawned {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    Ok(ConvertedScene {
        scene_bsn,
        report: ConversionReport {
            entity_count,
            asset_count,
        },
    })
}

/// Component types that are computed, structural, or transient and must not be
/// emitted as authored patches. Hierarchy is emitted structurally (via the AST
/// `Children` relation), and `Name` is emitted by `create_entity_in_ast`, so
/// both are excluded here. `SceneNodeId` is deliberately NOT excluded: it is
/// emitted as a plain component patch.
fn skip_type_ids() -> HashSet<TypeId> {
    HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
        TypeId::of::<Name>(),
        TypeId::of::<jackdaw_bsn::AstNodeRef>(),
        TypeId::of::<jackdaw_bsn::AstDirty>(),
        // Derived handle attached to GltfSource entities at load time; the
        // authored GltfSource is what persists.
        TypeId::of::<bevy::world_serialization::WorldAssetRoot>(),
    ])
}

/// Reflect every spawned entity into the fresh `SceneBsnAst` and emit it.
fn build_scene_bsn(
    world: &mut World,
    spawned: &[Entity],
    parent_path: &Path,
    asset_names: &bevy::platform::collections::HashMap<bevy::asset::UntypedAssetId, String>,
    asset_refs: &[CatalogAssetRef],
) -> String {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_server = world.resource::<AssetServer>().clone();
    let skip = skip_type_ids();

    if !asset_refs.is_empty() {
        let mut ast = world.remove_resource::<SceneBsnAst>().unwrap_or_default();
        jackdaw_bsn::append_assets_to_ast(&mut ast, world, asset_refs);
        world.insert_resource(ast);
    }

    // Parents must exist in the AST before their children so the hierarchy
    // relation resolves. Ordering by ancestor depth guarantees that.
    let order = parent_first_order(world, spawned);

    for entity in order {
        let parent = world
            .get::<ChildOf>(entity)
            .map(|c| c.parent())
            .filter(|p| spawned.contains(p));
        create_entity_in_ast(world, entity, parent);

        let Some(patches_entity) = world.resource::<SceneBsnAst>().ast_for(entity) else {
            continue;
        };

        // Reflect the entity's authored components into patches. Collect first
        // (immutable borrow of world), then write into the AST resource.
        let mut collected: Vec<(String, jackdaw_bsn::BsnPatch)> = Vec::new();
        {
            let reg = registry.read();
            let ctx = BsnAssetContext {
                asset_server: &asset_server,
                parent_path,
                asset_names: Some(asset_names),
            };
            let entity_ref = world.entity(entity);
            for registration in reg.iter() {
                if skip.contains(&registration.type_id()) {
                    continue;
                }
                let type_path = registration.type_info().type_path();
                if should_skip_component(type_path) {
                    continue;
                }
                let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(component) = reflect_component.reflect(entity_ref) else {
                    continue;
                };
                let patch =
                    component_to_bsn_patch_with_assets(component.as_partial_reflect(), &reg, &ctx);
                collected.push((type_path.to_string(), patch));
            }
        }

        let mut ast = world.resource_mut::<SceneBsnAst>();
        for (type_path, patch) in collected {
            if let Some(existing) = ast.find_patch_by_type_path(patches_entity, &type_path) {
                ast.set_patch(existing, patch);
            } else {
                let patch_entity = ast.world.spawn(patch).id();
                if let Some(patches) = ast.get_patches_mut(patches_entity) {
                    patches.0.push(patch_entity);
                }
            }
        }
    }

    emit_scene(world.resource::<SceneBsnAst>())
}

/// Order entities so every entity comes after its ancestors within the set.
/// Stable by original index within the same depth.
fn parent_first_order(world: &World, entities: &[Entity]) -> Vec<Entity> {
    let set: HashSet<Entity> = entities.iter().copied().collect();
    let mut order = entities.to_vec();
    order.sort_by_key(|&entity| {
        let mut depth = 0usize;
        let mut current = entity;
        while let Some(child_of) = world.get::<ChildOf>(current) {
            let parent = child_of.parent();
            if !set.contains(&parent) {
                break;
            }
            depth += 1;
            current = parent;
            if depth > entities.len() {
                break;
            }
        }
        depth
    });
    order
}

/// Turn resolved inline assets into catalog references, stripping the `#`/`@`
/// reference prefix so the catalog entry name is clean.
fn catalog_refs(local_assets: &HashMap<String, UntypedHandle>) -> Vec<CatalogAssetRef> {
    local_assets
        .iter()
        .map(|(name, handle)| {
            let clean = name.trim_start_matches(['#', '@']).to_string();
            CatalogAssetRef {
                name: clean,
                type_id: handle.type_id(),
                asset_id: handle.id(),
            }
        })
        .collect()
}

/// What [`convert_project`] did to a project directory.
#[derive(Default)]
pub struct ProjectConversionReport {
    /// Converted scene and prefab files (source path, per-scene report).
    pub scenes: Vec<(std::path::PathBuf, ConversionReport)>,
    /// Converted catalog files (source path).
    pub catalogs: Vec<std::path::PathBuf>,
    /// Files that failed to convert (source path, error). Failed files are
    /// left untouched.
    pub failures: Vec<(std::path::PathBuf, String)>,
}

/// Convert every `.jsn` scene, prefab, and catalog under `root` to `.bsn`,
/// renaming each converted source to `<name>.jsn.bak`.
///
/// Skipped: the `.jsn/` config directory (project settings move separately),
/// `project.jsn`, and existing `.jsn.bak` backups. Failures leave the source
/// untouched and are collected in the report.
pub fn convert_project(world: &mut World, root: &Path) -> ProjectConversionReport {
    let mut report = ProjectConversionReport::default();
    let mut files = Vec::new();
    collect_jsn_files(root, &mut files);

    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let result = if name == "catalog.jsn" {
            convert_catalog_file(world, &path).map(|()| report.catalogs.push(path.clone()))
        } else {
            convert_scene_file(world, &path)
                .map(|scene_report| report.scenes.push((path.clone(), scene_report)))
        };
        if let Err(err) = result {
            report.failures.push((path, err.to_string()));
        }
    }

    report
}

pub(crate) fn collect_jsn_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // `.jsn/` holds project config (which migrates separately), except
            // for the catalog, which is scene data.
            if dir_name == ".jsn" {
                let catalog = path.join("catalog.jsn");
                if catalog.is_file() {
                    out.push(catalog);
                }
                continue;
            }
            if dir_name.starts_with('.') || dir_name == "target" {
                continue;
            }
            collect_jsn_files(&path, out);
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".jsn") && name != "project.jsn" {
            out.push(path);
        }
    }
}

fn convert_scene_file(world: &mut World, path: &Path) -> Result<ConversionReport, BevyError> {
    let text = std::fs::read_to_string(path)?;
    let (scene, _version) = jackdaw_jsn::format::parse_scene(&text)
        .map_err(|e| BevyError::from(format!("could not parse {}: {e}", path.display())))?;
    let parent = path.parent().unwrap_or(Path::new(""));
    let converted = convert_jsn_scene_to_bsn_at(world, &scene, parent)?;

    let bsn_path = path.with_extension("bsn");
    std::fs::write(&bsn_path, &converted.scene_bsn)?;
    std::fs::rename(path, backup_path(path))?;
    Ok(converted.report)
}

fn convert_catalog_file(world: &mut World, path: &Path) -> Result<(), BevyError> {
    let text = std::fs::read_to_string(path)?;
    let catalog: jackdaw_jsn::format::JsnCatalog = serde_json::from_str(&text)
        .map_err(|e| BevyError::from(format!("could not parse {}: {e}", path.display())))?;

    // Catalog keys are reflect type paths; rewrite any legacy ones.
    let mut assets = jackdaw_jsn::format::JsnAssets::default();
    for (type_path, entries) in catalog.assets.0 {
        let key = jackdaw_jsn::format::canonical_type_path(&type_path).unwrap_or(type_path);
        assets.0.insert(key, entries);
    }

    let parent = path.parent().unwrap_or(Path::new(""));
    let local_assets = load_inline_assets(world, &assets, parent);
    let refs = catalog_refs(&local_assets);
    let catalog_bsn = serialize_assets_to_bsn(world, &refs);

    std::fs::write(path.with_extension("bsn"), catalog_bsn)?;
    std::fs::rename(path, backup_path(path))?;
    Ok(())
}

/// `scene.jsn` -> `scene.jsn.bak` (appends, so the original extension stays
/// visible in the backup name).
fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}
