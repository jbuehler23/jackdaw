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
    BsnAssetContext, BsnPatch, BsnPatches, BsnValue, SceneBsnAst, bsn_value_to_reflect,
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
    let ast = parse_bsn_text(bsn_text)?;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let server = world.get_resource::<AssetServer>().cloned();
    let assets_ctx = server.as_ref().map(|s| crate::BsnApplyAssets {
        server: s,
        local: None,
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

        let handle = reflect_asset.add(world, &*value);
        entries.push(CatalogEntry { name, handle });
    }

    Ok(entries)
}

/// Whether a document root is a named asset entry (its type patch resolves to
/// a registered `Asset` type). Scene loading routes these into `Assets<T>`
/// stores instead of spawning them as entities.
pub(crate) fn is_asset_root(
    ast: &SceneBsnAst,
    root: bevy::ecs::entity::Entity,
    reg: &bevy::reflect::TypeRegistry,
) -> bool {
    asset_value_from_root(ast, root)
        .and_then(|(type_path, _)| reg.get_with_type_path(&type_path))
        .is_some_and(|registration| registration.data::<ReflectAsset>().is_some())
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
    let ast = parse_bsn_text(text)?;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut assets = Vec::new();
    {
        let reg = registry.read();
        let roots = ast.roots.clone();
        for root in roots {
            if !is_asset_root(&ast, root, &reg) {
                continue;
            }
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

    // Record both reference spellings: scene-inline (`#`) and catalog (`@`).
    let mut names = bevy::platform::collections::HashMap::default();
    for entry in &assets {
        names.insert(format!("#{}", entry.name), entry.handle.clone());
        names.insert(format!("@{}", entry.name), entry.handle.clone());
    }
    world.insert_resource(crate::BsnSceneAssets(names));

    world.insert_resource(ast);
    let entities = crate::spawn_from_ast(world);
    crate::apply_dirty_ast_patches(world);

    Ok(LoadedBsnScene { entities, assets })
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
    let registration = reg.get_with_type_path(type_path)?;
    let reflect_asset = registration.data::<ReflectAsset>()?;
    let type_id = registration.type_id();

    let server = world.get_resource::<AssetServer>().cloned();
    let assets_ctx = server.as_ref().map(|s| crate::BsnApplyAssets {
        server: s,
        local: None,
    });
    let value = bsn_value_to_reflect(asset_value, type_id, reg, assets_ctx.as_ref())?;
    let handle = reflect_asset.add(world, &*value);
    Some(CatalogEntry {
        name: name.to_owned(),
        handle,
    })
}

/// Reconstruct the asset's type path and a [`BsnValue`] from an entry root's
/// non-name patch.
fn asset_value_from_root(
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
    let mut ast = SceneBsnAst::default();
    append_assets_to_ast(&mut ast, world, assets);
    emit_scene(&ast)
}

/// Append named asset entries to an existing document as roots, sorted by
/// name. Scene conversion uses this to embed a scene's inline assets in the
/// same `.bsn` document as its entities.
pub fn append_assets_to_ast(ast: &mut SceneBsnAst, world: &World, assets: &[CatalogAssetRef]) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let asset_server = world.get_resource::<AssetServer>();

    let mut sorted: Vec<&CatalogAssetRef> = assets.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    for asset_ref in sorted {
        let Some(registration) = reg.get(asset_ref.type_id) else {
            continue;
        };

        // Generic asset types cannot round-trip through the parser (their type
        // path is not a valid path token), so skip them.
        if registration.type_info().type_path().contains('<') {
            continue;
        }

        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            continue;
        };
        let Some(asset_value) = reflect_asset.get(world, asset_ref.asset_id) else {
            continue;
        };

        let patch = if let Some(server) = asset_server {
            let ctx = BsnAssetContext {
                asset_server: server,
                parent_path: Path::new(""),
                asset_names: None,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::{Asset, AssetApp, AssetPlugin, Assets};
    use bevy::prelude::ReflectDefault;
    use bevy::reflect::Reflect;

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
        assert_eq!(
            base_path.to_string().replace('\\', "/"),
            "textures/base.png"
        );

        let normal_handle = loaded.normal_map.expect("normal map should be Some");
        let normal_path = server
            .get_path(normal_handle.id())
            .expect("normal texture path");
        assert_eq!(
            normal_path.to_string().replace('\\', "/"),
            "textures/normal.png"
        );
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
