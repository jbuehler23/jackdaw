//! BSN asset catalog: load and save named asset definitions in `.bsn` text.
//!
//! A catalog is a `.bsn` document whose entries are named asset values. Each
//! entry pairs a `#Name` with a reflectable asset type (a `Struct`/`Type`
//! patch). [`load_bsn_assets`] parses catalog text and inserts the assets into
//! their `Assets<T>` stores via reflection, returning name-to-handle entries.
//! [`serialize_assets_to_bsn`] reflects named assets back out and emits catalog
//! text with default-diffing (only non-default fields are written), sorted by
//! name for deterministic output.

use std::any::TypeId;
use std::path::Path;

use bevy::asset::{AssetServer, ReflectAsset, UntypedAssetId, UntypedHandle};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::world::World;

use crate::{
    AssetNames, BsnAssetContext, BsnPatch, BsnPatches, BsnValue, SceneBsnAst, bsn_value_to_reflect,
    component_to_bsn_patch, component_to_bsn_patch_with_assets, emit_scene, parse_bsn_text,
};

pub use crate::loader::BsnLoadError;

/// A named asset entry produced by [`load_bsn_assets`].
pub struct CatalogEntry {
    /// The `#Name` from the BSN catalog entry.
    pub name: String,
    /// Handle to the asset created in its `Assets<T>` store.
    pub handle: UntypedHandle,
}

/// A named asset reference passed to [`serialize_assets_to_bsn`].
pub struct CatalogAssetRef {
    /// Display name for the asset (becomes `#Name` in the emitted catalog).
    pub name: String,
    /// The concrete asset type.
    pub type_id: TypeId,
    /// The asset's id in its `Assets<T>` store.
    pub asset_id: UntypedAssetId,
}

/// Parse catalog BSN text and insert each named asset into its `Assets<T>`
/// store via reflection.
///
/// The catalog may wrap its entries in a top-level `Children [ ... ]` relation
/// (as [`serialize_assets_to_bsn`] emits) or hold a single flat entry.
/// [`parse_bsn_text`] normalizes both into `SceneBsnAst::roots`, so each root
/// is one entry here.
pub fn load_bsn_assets(
    world: &mut World,
    bsn_text: &str,
) -> Result<Vec<CatalogEntry>, BsnLoadError> {
    load_bsn_assets_onto(world, bsn_text, |_| None)
}

/// [`load_bsn_assets`], writing an entry into the handle `held` answers for its
/// name when that handle is a live asset of the entry's type. Everything holding
/// the handle sees the new value. An entry with no such handle gets a new one.
pub fn load_bsn_assets_onto(
    world: &mut World,
    bsn_text: &str,
    held: impl Fn(&str) -> Option<UntypedHandle>,
) -> Result<Vec<CatalogEntry>, BsnLoadError> {
    let ast = parse_bsn_text(bsn_text)?;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let server = world.get_resource::<AssetServer>().cloned();
    let assets_ctx = server.as_ref().map(|s| crate::BsnApplyAssets {
        server: s,
        local: None,
        unresolved: None,
    });
    let reg = registry.read();

    let mut entries = Vec::new();

    for &root in &ast.roots {
        let Some(name) = ast.get_name(root).map(str::to_owned) else {
            continue;
        };
        let Some((type_path, asset_value)) = asset_value_from_root(&ast, root) else {
            continue;
        };

        let Some(registration) = reg.get_with_type_path(&type_path) else {
            continue;
        };
        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            continue;
        };
        let type_id = registration.type_id();

        let Some(value) = bsn_value_to_reflect(&asset_value, type_id, &reg, assets_ctx.as_ref())
        else {
            continue;
        };

        let replaced = held(&name)
            .filter(|handle| {
                handle.type_id() == type_id && reflect_asset.get(world, handle.id()).is_some()
            })
            .filter(|handle| reflect_asset.insert(world, handle.id(), &*value).is_ok());
        let handle = replaced.unwrap_or_else(|| reflect_asset.add(world, &*value));
        entries.push(CatalogEntry { name, handle });
    }

    Ok(entries)
}

/// Whether a document root is a named asset entry (its type patch resolves to
/// a registered `Asset` type). Scene loading routes these into `Assets<T>`
/// stores instead of spawning them as entities.
pub fn is_asset_root(
    ast: &SceneBsnAst,
    root: bevy::ecs::entity::Entity,
    reg: &bevy::reflect::TypeRegistry,
) -> bool {
    asset_value_from_root(ast, root)
        .and_then(|(type_path, _)| reg.get_with_type_path(&type_path))
        .is_some_and(|registration| registration.data::<ReflectAsset>().is_some())
}

/// Document roots that are named asset catalog entries.
pub fn asset_roots(
    ast: &SceneBsnAst,
    reg: &bevy::reflect::TypeRegistry,
) -> Vec<bevy::ecs::entity::Entity> {
    ast.roots
        .iter()
        .copied()
        .filter(|&root| is_asset_root(ast, root, reg))
        .collect()
}

/// Document roots that spawn as scene entities (not named asset entries).
pub fn entity_roots(
    ast: &SceneBsnAst,
    reg: &bevy::reflect::TypeRegistry,
) -> Vec<bevy::ecs::entity::Entity> {
    ast.roots
        .iter()
        .copied()
        .filter(|&root| !is_asset_root(ast, root, reg))
        .collect()
}

/// The result of loading a scene `.bsn`: spawned entities plus the named
/// assets that were embedded in the document.
pub struct LoadedBsnScene {
    pub entities: Vec<bevy::ecs::entity::Entity>,
    pub assets: Vec<CatalogEntry>,
}

/// Load scene `.bsn` text: embedded named asset entries go into their
/// `Assets<T>` stores (recorded in the [`crate::BsnSceneAssets`] resource so
/// `#Name`/`@Name` reference strings resolve during apply), entity roots spawn
/// into the world, and the parsed document becomes the live [`SceneBsnAst`].
pub fn load_bsn_scene(world: &mut World, text: &str) -> Result<LoadedBsnScene, BsnLoadError> {
    Ok(load_bsn_document(world, parse_bsn_text(text)?))
}

/// [`load_bsn_scene`] for a document already parsed, so a caller can parse
/// before clearing the scene it replaces.
pub fn load_bsn_document(world: &mut World, mut ast: SceneBsnAst) -> LoadedBsnScene {
    let _span = bevy::log::info_span!("doc.load").entered();
    ast.clear_links();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut assets = Vec::new();
    {
        let reg = registry.read();
        // A file written before list elements left out their defaults reads
        // the same as one written after, so values in it compare equal to the
        // values the live world writes back.
        bevy::log::info_span!("doc.compact").in_scope(|| crate::compact_document(&mut ast, &reg));
        let _assets = bevy::log::info_span!("doc.load_assets").entered();
        for root in asset_roots(&ast, &reg) {
            let Some(name) = ast.get_name(root).map(str::to_owned) else {
                continue;
            };
            let Some((type_path, asset_value)) = asset_value_from_root(&ast, root) else {
                continue;
            };
            let Some(entry) = load_asset_entry(world, &reg, &name, &type_path, &asset_value) else {
                continue;
            };
            assets.push(entry);
        }
    }

    // The project's asset files by path, then the document's own entries in
    // both spellings: scene-inline (`#`) and catalog (`@`).
    let mut names = world
        .get_resource::<crate::BsnProjectAssets>()
        .map(|project| project.0.clone())
        .unwrap_or_default();
    for entry in &assets {
        if names
            .insert(format!("#{}", entry.name), entry.handle.clone())
            .is_some()
        {
            bevy::log::warn!(
                "the scene defines `#{}` more than once; every use reads the last definition",
                entry.name
            );
        }
        names.insert(format!("@{}", entry.name), entry.handle.clone());
    }
    world.insert_resource(crate::BsnSceneAssets(names));

    world.insert_resource(ast);
    if let Some(mut unresolved) = world.get_resource_mut::<crate::UnresolvedTypes>() {
        unresolved.start_scene();
    }
    let entities = bevy::log::info_span!("doc.spawn").in_scope(|| crate::spawn_from_ast(world));
    bevy::log::info_span!("doc.apply").in_scope(|| crate::apply_dirty_ast_patches(world));
    report_unresolved_types(world);

    LoadedBsnScene { entities, assets }
}

/// Reports once, after a load, that the editor's picture of the project is
/// older than the scene and a rebuild refreshes it.
fn report_unresolved_types(world: &mut World) {
    let Some(mut unresolved) = world.get_resource_mut::<crate::UnresolvedTypes>() else {
        return;
    };
    if let Some(remedy) = unresolved.take_remedy() {
        log::warn!("{remedy}");
    }
}

/// Adopts `source`'s named asset entries into the open scene.
///
/// Each entry's value goes into its `Assets<T>` store, both reference spellings
/// join [`crate::BsnSceneAssets`], and the entry joins the live document as a
/// root unless a root of that name is already there.
///
/// An entry whose name the scene already uses for the same value reuses the
/// scene's asset. One whose name the scene uses for a different value is
/// adopted under a new name, and `source`'s references to it are renamed to
/// match. Returns the names of the entries that could not be adopted.
pub fn adopt_asset_roots(world: &mut World, source: &mut SceneBsnAst) -> Vec<String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let roots = {
        let reg = registry.read();
        asset_roots(source, &reg)
    };
    if roots.is_empty() {
        return Vec::new();
    }

    let published = world
        .get_resource::<crate::BsnSceneAssets>()
        .map(|assets| assets.0.clone())
        .unwrap_or_default();
    let mut joining = Vec::new();
    let mut renames = Vec::new();
    {
        let live = world.resource::<SceneBsnAst>();
        let mut taken = names_in_document(live);
        taken.extend(names_in_document(source));
        taken.extend(
            published
                .keys()
                .filter_map(|key| key.strip_prefix('#'))
                .map(str::to_owned),
        );
        for root in roots {
            let Some(name) = source.get_name(root) else {
                continue;
            };
            let existing = live
                .roots
                .iter()
                .copied()
                .find(|&existing| live.get_name(existing) == Some(name));
            match existing {
                None => joining.push(root),
                Some(existing)
                    if compact_asset_value(live, existing, &registry)
                        == compact_asset_value(source, root, &registry) =>
                {
                    if !published.contains_key(&format!("#{name}")) {
                        joining.push(root);
                    }
                }
                Some(_) => {
                    renames.push((root, name.to_owned(), unused_name(name, &mut taken)));
                    joining.push(root);
                }
            }
        }
    }
    for (root, old, new) in renames {
        rename_asset_root(source, root, &old, &new);
    }

    let mut dropped = Vec::new();
    let mut fresh = Vec::new();
    for root in joining {
        let Some(name) = source.get_name(root).map(str::to_owned) else {
            continue;
        };
        let value = asset_value_from_root(source, root);
        let entry = value.and_then(|(type_path, asset_value)| {
            let reg = registry.read();
            load_asset_entry(world, &reg, &name, &type_path, &asset_value)
        });
        match entry {
            Some(entry) => fresh.push((root, entry)),
            None => dropped.push(name),
        }
    }

    let mut names = published;
    for (_, entry) in &fresh {
        names.insert(format!("#{}", entry.name), entry.handle.clone());
        names.insert(format!("@{}", entry.name), entry.handle.clone());
    }
    world.insert_resource(crate::BsnSceneAssets(names));

    let mut live = world.resource_mut::<SceneBsnAst>();
    for (root, entry) in fresh {
        let present = live
            .roots
            .iter()
            .any(|&existing| live.get_name(existing) == Some(entry.name.as_str()));
        if !present {
            crate::clone_subtree_into(&mut live, source, root, None);
        }
    }
    dropped
}

/// Every `#Name` a document's nodes carry.
fn names_in_document(ast: &SceneBsnAst) -> std::collections::HashSet<String> {
    ast.roots
        .iter()
        .flat_map(|&root| std::iter::once(root).chain(ast.descendants_of(root)))
        .filter_map(|node| ast.get_name(node).map(str::to_owned))
        .collect()
}

/// `name` with its trailing number replaced by the lowest one not in `taken`,
/// which then holds it.
fn unused_name(name: &str, taken: &mut std::collections::HashSet<String>) -> String {
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    (0_usize..)
        .map(|n| format!("{stem}{n}"))
        .find(|candidate| taken.insert(candidate.clone()))
        .expect("an unused name exists")
}

/// Rename the asset at `root` from `old` to `new`, and every reference to it in
/// `ast`'s component values.
fn rename_asset_root(ast: &mut SceneBsnAst, root: bevy::ecs::entity::Entity, old: &str, new: &str) {
    let name_patch = ast.get_patches(root).and_then(|patches| {
        patches
            .0
            .iter()
            .copied()
            .find(|&patch| matches!(ast.get_patch(patch), Some(BsnPatch::Name(_))))
    });
    if let Some(patch) = name_patch {
        ast.set_patch(patch, BsnPatch::Name(new.to_owned()));
    }
    let spellings = [
        (format!("#{old}"), format!("#{new}")),
        (format!("@{old}"), format!("@{new}")),
    ];
    let mut patches = ast.world.query::<&mut BsnPatch>();
    for mut patch in patches.iter_mut(&mut ast.world) {
        match &mut *patch {
            BsnPatch::Struct(data) => {
                for field in &mut data.fields.0 {
                    rename_in_value(&mut field.value, &spellings);
                }
            }
            BsnPatch::TupleStruct(data) => {
                for value in &mut data.values {
                    rename_in_value(value, &spellings);
                }
            }
            BsnPatch::Template(_, Some(fields)) => {
                for field in &mut fields.0 {
                    rename_in_value(&mut field.value, &spellings);
                }
            }
            _ => {}
        }
    }
}

fn rename_in_value(value: &mut BsnValue, spellings: &[(String, String)]) {
    match value {
        BsnValue::String(text) => {
            if let Some((_, to)) = spellings.iter().find(|(from, _)| from == text) {
                text.clone_from(to);
            }
        }
        BsnValue::Struct(data) => {
            for field in &mut data.fields.0 {
                rename_in_value(&mut field.value, spellings);
            }
        }
        BsnValue::TupleStruct(data) => {
            for value in &mut data.values {
                rename_in_value(value, spellings);
            }
        }
        BsnValue::List(values) => {
            for value in values {
                rename_in_value(value, spellings);
            }
        }
        BsnValue::Map(pairs) => {
            for (_, value) in pairs {
                rename_in_value(value, spellings);
            }
        }
        _ => {}
    }
}

/// Load the asset one document root holds into its `Assets<T>` store.
pub fn load_asset_root(
    world: &mut World,
    ast: &SceneBsnAst,
    root: bevy::ecs::entity::Entity,
) -> Option<UntypedHandle> {
    let (type_path, asset_value) = asset_value_from_root(ast, root)?;
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    load_asset_value(world, &reg, &type_path, &asset_value)
}

/// Build one named asset from its document value and insert it into its
/// `Assets<T>` store.
fn load_asset_entry(
    world: &mut World,
    reg: &bevy::reflect::TypeRegistry,
    name: &str,
    type_path: &str,
    asset_value: &BsnValue,
) -> Option<CatalogEntry> {
    load_asset_value(world, reg, type_path, asset_value).map(|handle| CatalogEntry {
        name: name.to_owned(),
        handle,
    })
}

/// Build one asset from its document value and insert it into its `Assets<T>`
/// store.
fn load_asset_value(
    world: &mut World,
    reg: &bevy::reflect::TypeRegistry,
    type_path: &str,
    asset_value: &BsnValue,
) -> Option<UntypedHandle> {
    let registration = reg.get_with_type_path(type_path)?;
    let reflect_asset = registration.data::<ReflectAsset>()?;
    let type_id = registration.type_id();

    let server = world.get_resource::<AssetServer>().cloned();
    let assets_ctx = server.as_ref().map(|s| crate::BsnApplyAssets {
        server: s,
        local: None,
        unresolved: None,
    });
    let value = bsn_value_to_reflect(asset_value, type_id, reg, assets_ctx.as_ref())?;
    Some(reflect_asset.add(world, &*value))
}

/// [`asset_value_from_root`] in the form live values take, so two spellings of
/// one value compare equal.
fn compact_asset_value(
    ast: &SceneBsnAst,
    root: bevy::ecs::entity::Entity,
    registry: &AppTypeRegistry,
) -> Option<(String, BsnValue)> {
    let (type_path, mut value) = asset_value_from_root(ast, root)?;
    crate::document::compact_value(&mut value, &registry.read());
    Some((type_path, value))
}

/// The type path and value a document root's first non-name patch names.
pub fn asset_value_from_root(
    ast: &SceneBsnAst,
    root: bevy::ecs::entity::Entity,
) -> Option<(String, BsnValue)> {
    let patches = ast.get_patches(root)?;
    for &pe in &patches.0 {
        match ast.get_patch(pe)? {
            BsnPatch::Struct(data) => {
                return Some((data.type_path.clone(), BsnValue::Struct(data.clone())));
            }
            BsnPatch::TupleStruct(data) => {
                return Some((data.type_path.clone(), BsnValue::TupleStruct(data.clone())));
            }
            BsnPatch::Type(type_path) => {
                return Some((type_path.clone(), BsnValue::Type(type_path.clone())));
            }
            _ => {}
        }
    }
    None
}

/// Serialize named assets to catalog BSN text.
///
/// Each asset is reflected from its `Assets<T>` store, default-diffed, and
/// emitted with `Handle<T>` and `Option<Handle<T>>` fields resolved to asset
/// paths (when an [`AssetServer`] is present). Entries are sorted by name so
/// the output is stable across calls.
pub fn serialize_assets_to_bsn(world: &World, assets: &[CatalogAssetRef]) -> String {
    serialize_assets_to_bsn_reporting(world, assets).0
}

/// [`serialize_assets_to_bsn`] plus the names it could not emit.
///
/// Emission drops an entry silently when its type is unregistered, carries no
/// `ReflectAsset`, or the asset is gone from its store. The text alone does not
/// distinguish "nothing to write" from "everything skipped", so a caller
/// deciding a file's fate gets the skipped names here.
pub fn serialize_assets_to_bsn_reporting(
    world: &World,
    assets: &[CatalogAssetRef],
) -> (String, Vec<String>) {
    let mut ast = SceneBsnAst::default();
    let skipped = append_assets_to_ast(&mut ast, world, assets);
    (emit_scene(&ast), skipped)
}

/// Append named asset entries to an existing document as roots, sorted by
/// name. Scene conversion uses this to embed a scene's inline assets in the
/// same `.bsn` document as its entities.
///
/// Returns the names of entries that could not be emitted.
pub fn append_assets_to_ast(
    ast: &mut SceneBsnAst,
    world: &World,
    assets: &[CatalogAssetRef],
) -> Vec<String> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();
    let paths = world.get_resource::<crate::BsnAssetPaths>();

    let mut sorted: Vec<&CatalogAssetRef> = assets.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut skipped = Vec::new();
    for asset_ref in sorted {
        let Some(registration) = reg.get(asset_ref.type_id) else {
            skipped.push(asset_ref.name.clone());
            continue;
        };

        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            skipped.push(asset_ref.name.clone());
            continue;
        };
        let Some(asset_value) = reflect_asset.get(world, asset_ref.asset_id) else {
            skipped.push(asset_ref.name.clone());
            continue;
        };

        let patch = if let Some(server) = asset_server {
            let ctx = BsnAssetContext {
                asset_server: server,
                parent_path: Path::new(""),
                asset_names: paths.as_ref().map(|paths| &paths.0 as &dyn AssetNames),
            };
            component_to_bsn_patch_with_assets(asset_value.as_partial_reflect(), &reg, &ctx)
        } else {
            component_to_bsn_patch(asset_value.as_partial_reflect(), &reg)
        };

        let name_patch = ast.world.spawn(BsnPatch::Name(asset_ref.name.clone())).id();
        let type_patch = ast.world.spawn(patch).id();
        let root = ast
            .world
            .spawn(BsnPatches(vec![name_patch, type_patch]))
            .id();
        ast.add_to_roots(root);
    }
    skipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::{Asset, AssetApp, AssetPlugin, Assets};
    use bevy::prelude::ReflectDefault;
    use bevy::reflect::Reflect;
    use path_slash::PathExt as _;

    // A small asset with only scalar fields. Round-tripping this needs no
    // asset server, exercising the plain reflect path end to end.
    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct TestMaterial {
        metallic: f32,
        roughness: f32,
    }

    fn scalar_world() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut w = registry.write();
            w.register::<TestMaterial>();
            w.register_type_data::<TestMaterial, ReflectAsset>();
        }
        world.insert_resource(registry);
        world.insert_resource(Assets::<TestMaterial>::default());
        world
    }

    fn get_material<'w>(world: &'w World, handle: &UntypedHandle) -> &'w TestMaterial {
        world
            .resource::<Assets<TestMaterial>>()
            .get(&handle.clone().typed::<TestMaterial>())
            .expect("asset should exist")
    }

    #[test]
    fn loading_onto_a_held_handle_writes_the_value_into_its_asset() {
        let mut world = scalar_world();
        let path = "jackdaw_bsn::catalog::tests::TestMaterial";
        let first = load_bsn_assets(&mut world, &format!("#Stone\n{path} {{ metallic: 0.5 }}\n"))
            .expect("it parses")
            .remove(0)
            .handle;
        let text = format!("#Stone\n{path} {{ metallic: 0.75 }}\n");

        let held = first.clone();
        let onto = load_bsn_assets_onto(&mut world, &text, |_| Some(held.clone()))
            .expect("it parses")
            .remove(0)
            .handle;
        assert_eq!(onto.id(), first.id());
        assert_eq!(get_material(&world, &first).metallic, 0.75);

        let fresh = load_bsn_assets_onto(&mut world, &text, |_| None)
            .expect("it parses")
            .remove(0)
            .handle;
        assert_ne!(fresh.id(), first.id(), "with nothing held, a new asset");
    }

    #[derive(Reflect, Clone, Default)]
    #[reflect(Default)]
    struct TestLayer {
        weight: f32,
        tint: f32,
    }

    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct TestLayers {
        layers: Vec<TestLayer>,
    }

    /// Older clipboard text spells every field of a list element and widens
    /// its floats; it is still the value the scene holds.
    #[test]
    fn older_text_of_an_asset_the_scene_holds_is_adopted_as_the_same_asset() {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut w = registry.write();
            w.register::<TestLayers>();
            w.register::<TestLayer>();
            w.register::<Vec<TestLayer>>();
            w.register_type_data::<TestLayers, ReflectAsset>();
        }
        world.insert_resource(registry);
        world.insert_resource(Assets::<TestLayers>::default());
        world.insert_resource(SceneBsnAst::default());

        let handle = world.resource_mut::<Assets<TestLayers>>().add(TestLayers {
            layers: vec![TestLayer {
                weight: 0.0,
                tint: 0.1,
            }],
        });
        let mut current = SceneBsnAst::default();
        append_assets_to_ast(
            &mut current,
            &world,
            &[CatalogAssetRef {
                name: "Shared".into(),
                type_id: TypeId::of::<TestLayers>(),
                asset_id: handle.id().untyped(),
            }],
        );
        assert!(adopt_asset_roots(&mut world, &mut current).is_empty());

        let path = "jackdaw_bsn::catalog::tests";
        let mut older = parse_bsn_text(&format!(
            "#Shared\n{path}::TestLayers {{ layers: [{path}::TestLayer {{ weight: 0.0, tint: {} }}] }}\n",
            f64::from(0.1_f32)
        ))
        .expect("it parses");
        assert!(adopt_asset_roots(&mut world, &mut older).is_empty());

        let mut other = parse_bsn_text(&format!(
            "#Shared\n{path}::TestLayers {{ layers: [{path}::TestLayer {{ tint: 0.75 }}] }}\n"
        ))
        .expect("it parses");
        assert!(adopt_asset_roots(&mut world, &mut other).is_empty());
        assert_eq!(
            world.resource::<SceneBsnAst>().roots.len(),
            2,
            "a different value joins under a name of its own"
        );
    }

    /// A copied subtree carries the assets its components name, and paste
    /// adopts them into the store, the reference names and the document.
    #[test]
    fn adopting_a_clipboard_document_s_assets_makes_its_references_resolvable() {
        let mut world = scalar_world();
        world.insert_resource(SceneBsnAst::default());

        let handle = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial {
                metallic: 0.5,
                roughness: 0.25,
            });
        let mut source = SceneBsnAst::default();
        append_assets_to_ast(
            &mut source,
            &world,
            &[CatalogAssetRef {
                name: "Copied".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: handle.id().untyped(),
            }],
        );
        world.resource_mut::<Assets<TestMaterial>>().remove(&handle);

        let dropped = adopt_asset_roots(&mut world, &mut source);

        assert!(dropped.is_empty(), "nothing was dropped: {dropped:?}");
        let names = &world.resource::<crate::BsnSceneAssets>().0;
        let adopted = names
            .get("#Copied")
            .expect("the inline reference name resolves");
        assert!(
            names.contains_key("@Copied"),
            "and the catalog spelling too"
        );
        assert_eq!(get_material(&world, adopted).roughness, 0.25);
        assert_eq!(
            world.resource::<SceneBsnAst>().roots.len(),
            1,
            "the entry joined the open document, so a save keeps it"
        );

        adopt_asset_roots(&mut world, &mut source);
        assert_eq!(world.resource::<SceneBsnAst>().roots.len(), 1);
    }

    /// A different value under a name the scene already uses joins under a new
    /// name, the references that came with it follow, and the name keeps reaching
    /// the asset already there.
    #[test]
    fn a_second_entry_of_the_same_name_joins_under_a_new_name_unless_it_is_the_same_asset() {
        let mut world = scalar_world();
        world.insert_resource(SceneBsnAst::default());

        let source_of = |world: &mut World, roughness: f32| {
            let handle = world
                .resource_mut::<Assets<TestMaterial>>()
                .add(TestMaterial {
                    metallic: 0.5,
                    roughness,
                });
            let mut ast = SceneBsnAst::default();
            append_assets_to_ast(
                &mut ast,
                world,
                &[CatalogAssetRef {
                    name: "Shared".into(),
                    type_id: TypeId::of::<TestMaterial>(),
                    asset_id: handle.id().untyped(),
                }],
            );
            ast
        };

        let mut first = source_of(&mut world, 0.25);
        assert!(adopt_asset_roots(&mut world, &mut first).is_empty());
        let published = world.resource::<crate::BsnSceneAssets>().0["#Shared"].clone();
        assert_eq!(get_material(&world, &published).roughness, 0.25);

        let mut again = source_of(&mut world, 0.25);
        assert!(
            adopt_asset_roots(&mut world, &mut again).is_empty(),
            "the same value under the same name is the same asset",
        );
        assert_eq!(world.resource::<SceneBsnAst>().roots.len(), 1);

        assert_eq!(
            world.resource::<crate::BsnSceneAssets>().0["#Shared"],
            published,
            "the same value reuses the asset the scene holds",
        );

        let mut other = source_of(&mut world, 0.75);
        let user =
            other.create_entity_node(vec![BsnPatch::TupleStruct(crate::BsnTupleStructData {
                type_path: "Uses".to_string(),
                values: vec![BsnValue::String("#Shared".to_string())],
            })]);
        other.add_to_roots(user);
        assert!(adopt_asset_roots(&mut world, &mut other).is_empty());
        assert_eq!(
            world.resource::<SceneBsnAst>().roots.len(),
            2,
            "the different value joined the document",
        );
        let names = &world.resource::<crate::BsnSceneAssets>().0;
        assert_eq!(get_material(&world, &names["#Shared0"]).roughness, 0.75);
        let Some(BsnPatch::TupleStruct(data)) = other
            .get_patches(user)
            .and_then(|patches| other.get_patch(patches.0[0]))
        else {
            panic!("the user keeps its patch");
        };
        assert!(
            matches!(&data.values[0], BsnValue::String(text) if text == "#Shared0"),
            "the reference that came with it follows the new name",
        );
        assert_eq!(
            get_material(
                &world,
                &world.resource::<crate::BsnSceneAssets>().0["#Shared"]
            )
            .roughness,
            0.25,
            "and the name still reaches the asset the scene was already using",
        );
    }

    /// One asset per file, named by its file stem, into a fresh world.
    #[test]
    fn a_directory_of_named_assets_round_trips_one_file_at_a_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut world = scalar_world();

        let written = [("Shiny", 1.0, 0.05), ("Rough", 0.0, 0.9)];
        for (name, metallic, roughness) in written {
            let handle = world
                .resource_mut::<Assets<TestMaterial>>()
                .add(TestMaterial {
                    metallic,
                    roughness,
                });
            let text = serialize_assets_to_bsn(
                &world,
                &[CatalogAssetRef {
                    name: name.to_string(),
                    type_id: TypeId::of::<TestMaterial>(),
                    asset_id: handle.id().untyped(),
                }],
            );
            std::fs::write(dir.path().join(format!("{name}.material.bsn")), text)
                .expect("the file is written");
        }

        let mut fresh = scalar_world();
        let mut loaded: Vec<(String, f32)> = Vec::new();
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
            .expect("the directory is there")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        files.sort();
        for path in files {
            let stem = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".material.bsn"))
                .expect("a file of this suffix")
                .to_string();
            let text = std::fs::read_to_string(&path).expect("the file reads");
            let entries = load_bsn_assets(&mut fresh, &text).expect("load should succeed");
            assert_eq!(entries.len(), 1, "a definition file holds one asset");
            assert_eq!(entries[0].name, stem, "the entry is named after its file");
            loaded.push((stem, get_material(&fresh, &entries[0].handle).roughness));
        }

        assert_eq!(
            loaded,
            vec![("Rough".to_string(), 0.9), ("Shiny".to_string(), 0.05)]
        );
    }

    #[test]
    fn round_trips_scalar_catalog_by_name_and_value() {
        let mut world = scalar_world();

        let a = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial {
                metallic: 1.0,
                roughness: 0.05,
            });
        let b = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial {
                metallic: 0.0,
                roughness: 0.9,
            });

        let refs = vec![
            CatalogAssetRef {
                name: "Shiny".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: a.id().untyped(),
            },
            CatalogAssetRef {
                name: "Rough".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: b.id().untyped(),
            },
        ];

        let text = serialize_assets_to_bsn(&world, &refs);

        // Load into a fresh world so nothing carries over.
        let mut fresh = scalar_world();
        let entries = load_bsn_assets(&mut fresh, &text).expect("load should succeed");

        assert_eq!(entries.len(), 2);
        // Sorted by name in the emitted text: Rough before Shiny.
        let by_name: std::collections::HashMap<&str, &UntypedHandle> = entries
            .iter()
            .map(|e| (e.name.as_str(), &e.handle))
            .collect();

        let rough = get_material(&fresh, by_name["Rough"]);
        assert!((rough.metallic - 0.0).abs() < f32::EPSILON);
        assert!((rough.roughness - 0.9).abs() < f32::EPSILON);

        let shiny = get_material(&fresh, by_name["Shiny"]);
        assert!((shiny.metallic - 1.0).abs() < f32::EPSILON);
        assert!((shiny.roughness - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn serialized_text_is_name_sorted_and_deterministic() {
        let mut world = scalar_world();

        let a = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial {
                metallic: 1.0,
                roughness: 0.5,
            });
        let b = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial {
                metallic: 0.2,
                roughness: 0.7,
            });

        // Pass the refs in non-sorted order.
        let refs = vec![
            CatalogAssetRef {
                name: "Zebra".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: a.id().untyped(),
            },
            CatalogAssetRef {
                name: "Apple".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: b.id().untyped(),
            },
        ];

        let first = serialize_assets_to_bsn(&world, &refs);
        let second = serialize_assets_to_bsn(&world, &refs);
        assert_eq!(first, second, "output must be byte-stable across calls");

        let apple = first.find("#Apple").expect("Apple present");
        let zebra = first.find("#Zebra").expect("Zebra present");
        assert!(apple < zebra, "entries must be sorted by name");
    }

    #[test]
    fn an_unemittable_entry_is_reported_not_dropped_silently() {
        let mut world = scalar_world();
        let real = world
            .resource_mut::<Assets<TestMaterial>>()
            .add(TestMaterial::default());

        let refs = vec![
            CatalogAssetRef {
                name: "Real".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: real.id().untyped(),
            },
            // An id whose asset is not in any store: nothing to reflect.
            CatalogAssetRef {
                name: "Gone".into(),
                type_id: TypeId::of::<TestMaterial>(),
                asset_id: bevy::asset::AssetId::<TestMaterial>::default().untyped(),
            },
        ];

        let (text, skipped) = serialize_assets_to_bsn_reporting(&world, &refs);
        assert!(text.contains("#Real"));
        assert_eq!(skipped, vec!["Gone".to_string()]);
    }

    #[test]
    fn malformed_catalog_returns_err_not_panic() {
        let mut world = scalar_world();
        let result = load_bsn_assets(&mut world, "this is not $$ valid bsn {{{");
        assert!(result.is_err(), "malformed catalog should return Err");
    }

    // ------------------------------------------------------------------
    // Handle / Option<Handle> round-trip (needs an AssetServer for paths).
    // ------------------------------------------------------------------

    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct Texture {
        _size: u32,
    }

    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct TexturedMaterial {
        tint: f32,
        base_color_texture: bevy::asset::Handle<Texture>,
        normal_map: Option<bevy::asset::Handle<Texture>>,
    }

    fn asset_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Texture>();
        app.init_asset::<TexturedMaterial>();
        app.register_asset_reflect::<Texture>();
        app.register_asset_reflect::<TexturedMaterial>();
        app
    }

    #[test]
    fn handle_and_option_handle_fields_round_trip_as_paths() {
        let mut app = asset_app();

        let (base, normal) = {
            let server = app.world().resource::<AssetServer>().clone();
            (
                server.load::<Texture>("textures/base.png"),
                server.load::<Texture>("textures/normal.png"),
            )
        };

        let id = app
            .world_mut()
            .resource_mut::<Assets<TexturedMaterial>>()
            .add(TexturedMaterial {
                tint: 0.5,
                base_color_texture: base,
                normal_map: Some(normal),
            });

        let refs = vec![CatalogAssetRef {
            name: "Painted".into(),
            type_id: TypeId::of::<TexturedMaterial>(),
            asset_id: id.id().untyped(),
        }];

        let text = serialize_assets_to_bsn(app.world(), &refs);
        assert!(
            text.contains("textures/base.png"),
            "handle field must serialize as an asset path, got:\n{text}"
        );
        assert!(
            text.contains("textures/normal.png"),
            "Option<Handle> field must serialize as an asset path, got:\n{text}"
        );

        let entries = load_bsn_assets(app.world_mut(), &text).expect("load should succeed");
        assert_eq!(entries.len(), 1);

        let loaded = {
            let handle = entries[0].handle.clone().typed::<TexturedMaterial>();
            app.world()
                .resource::<Assets<TexturedMaterial>>()
                .get(&handle)
                .expect("loaded material")
                .clone()
        };

        let server = app.world().resource::<AssetServer>();
        let base_path = server
            .get_path(loaded.base_color_texture.id())
            .expect("base texture path");
        assert_eq!(base_path.path().to_slash_lossy(), "textures/base.png");

        let normal_handle = loaded.normal_map.expect("normal map should be Some");
        let normal_path = server
            .get_path(normal_handle.id())
            .expect("normal texture path");
        assert_eq!(normal_path.path().to_slash_lossy(), "textures/normal.png");
    }

    // An asset whose `Option<Handle>` field defaults to `Some`, so a `None`
    // value differs from default and is actually emitted (rather than diffed
    // away). This asserts the None direction of the round trip end to end.
    #[derive(Asset, Reflect, Clone)]
    #[reflect(Default)]
    struct OptionalTextureMaterial {
        normal_map: Option<bevy::asset::Handle<Texture>>,
    }

    impl Default for OptionalTextureMaterial {
        fn default() -> Self {
            // Default is Some so a None value is a non-default override.
            Self {
                normal_map: Some(bevy::asset::Handle::default()),
            }
        }
    }

    #[test]
    fn option_handle_none_field_round_trips_as_none() {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Texture>();
        app.init_asset::<OptionalTextureMaterial>();
        app.register_asset_reflect::<Texture>();
        app.register_asset_reflect::<OptionalTextureMaterial>();

        let id = app
            .world_mut()
            .resource_mut::<Assets<OptionalTextureMaterial>>()
            .add(OptionalTextureMaterial { normal_map: None });

        let refs = vec![CatalogAssetRef {
            name: "NoNormal".into(),
            type_id: TypeId::of::<OptionalTextureMaterial>(),
            asset_id: id.id().untyped(),
        }];

        let text = serialize_assets_to_bsn(app.world(), &refs);

        let entries = load_bsn_assets(app.world_mut(), &text).expect("load should succeed");
        assert_eq!(entries.len(), 1);

        let handle = entries[0].handle.clone().typed::<OptionalTextureMaterial>();
        let loaded = app
            .world()
            .resource::<Assets<OptionalTextureMaterial>>()
            .get(&handle)
            .expect("loaded material")
            .clone();

        assert!(
            loaded.normal_map.is_none(),
            "a None Option<Handle> must reload as None, got Some"
        );
    }
}
