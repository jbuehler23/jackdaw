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

/// The result of converting a `JsnScene`: the scene `.bsn` text, the asset
/// catalog `.bsn` text, and a short report of what was converted.
pub struct ConvertedScene {
    /// Scene entities and components as `.bsn` text.
    pub scene_bsn: String,
    /// Named inline assets as catalog `.bsn` text (empty when there are none).
    pub catalog_bsn: String,
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
    let scene: JsnScene = match serde_json::from_str(text) {
        Ok(scene) => scene,
        Err(v3_err) => match serde_json::from_str::<jackdaw_jsn::format::JsnSceneV2>(text) {
            Ok(v2) => v2.migrate_to_v3(),
            Err(_) => return Err(BevyError::from(format!("could not parse .jsn: {v3_err}"))),
        },
    };
    convert_jsn_scene_to_bsn(world, &scene)
}

/// `convert_jsn_scene_to_bsn` with an explicit `parent_path`, the directory the
/// emitted `.bsn` would live in. Asset paths are emitted relative to it.
pub fn convert_jsn_scene_to_bsn_at(
    world: &mut World,
    scene: &JsnScene,
    parent_path: &Path,
) -> Result<ConvertedScene, BevyError> {
    // Resolve inline assets first so component handles can bind to them, then
    // spawn the scene with the Handle-aware loader (materials/textures resolve
    // to real handles instead of the null placeholder JSON stores for them).
    let local_assets = load_inline_assets(world, &scene.assets, parent_path);
    let spawned = load_scene_from_jsn(world, &scene.scene, parent_path, &local_assets);

    // Build the scene document in a fresh AST so the editor's live document is
    // untouched. Restore whatever was there afterward.
    let saved_ast = world.remove_resource::<SceneBsnAst>();
    world.insert_resource(SceneBsnAst::default());

    let scene_bsn = build_scene_bsn(world, &spawned, parent_path);

    world.remove_resource::<SceneBsnAst>();
    if let Some(ast) = saved_ast {
        world.insert_resource(ast);
    }

    // Catalog from the resolved inline assets.
    let asset_refs = catalog_refs(&local_assets);
    let catalog_bsn = if asset_refs.is_empty() {
        String::new()
    } else {
        serialize_assets_to_bsn(world, &asset_refs)
    };
    let asset_count = catalog_bsn.matches('#').count();

    let entity_count = spawned.len();

    // Despawn the transient entities so the conversion leaves no residue.
    for &entity in &spawned {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    Ok(ConvertedScene {
        scene_bsn,
        catalog_bsn,
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
    ])
}

/// Reflect every spawned entity into the fresh `SceneBsnAst` and emit it.
fn build_scene_bsn(world: &mut World, spawned: &[Entity], parent_path: &Path) -> String {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_server = world.resource::<AssetServer>().clone();
    let skip = skip_type_ids();

    // Parents must exist in the AST before their children so the hierarchy
    // relation resolves. Ordering by ancestor depth guarantees that.
    let order = parent_first_order(world, spawned);

    for entity in order {
        let parent = world
            .get::<ChildOf>(entity)
            .map(|c| c.parent())
            .filter(|p| spawned.contains(p));
        create_entity_in_ast(world, entity, parent);

        let Some(patches_entity) = world
            .resource::<SceneBsnAst>()
            .ast_for(entity)
        else {
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
                let patch = component_to_bsn_patch_with_assets(
                    component.as_partial_reflect(),
                    &reg,
                    &ctx,
                );
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
