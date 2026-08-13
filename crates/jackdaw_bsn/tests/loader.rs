//! Loader tests: `.bsn` text through to the document [`SceneBsnAst`].

use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst, parse_bsn_text};

use bevy::ecs::entity::Entity;

/// Collect every patch component on a patches entity.
fn patches_of<'a>(ast: &'a SceneBsnAst, node: Entity) -> Vec<&'a BsnPatch> {
    ast.get_patches(node)
        .expect("patches node")
        .0
        .iter()
        .map(|&pe| ast.get_patch(pe).expect("patch component"))
        .collect()
}

#[test]
fn loads_example_fixture() {
    let text = include_str!("fixtures/example.bsn");
    let ast = parse_bsn_text(text).expect("example.bsn should load");

    // The top-level entity is a real entity (name + components + Children), so
    // it stays a single root rather than being unwrapped as a children list.
    assert_eq!(ast.roots.len(), 1, "root count");
    let root = ast.roots[0];

    // The `#Root` name is carried on the root node.
    assert_eq!(ast.get_name(root), Some("Root"), "root name");

    let root_patches = patches_of(&ast, root);
    assert_eq!(root_patches.len(), 4, "root patch count");

    // A bare component type patch: Transform.
    assert!(
        root_patches.iter().any(|p| matches!(
            p,
            BsnPatch::Type(tp) if tp.ends_with("Transform")
        )),
        "expected a Type patch for Transform"
    );

    // An enum unit variant patch: Visibility::Visible.
    assert!(
        root_patches.iter().any(|p| matches!(
            p,
            BsnPatch::Type(tp) if tp.ends_with("Visibility::Visible")
        )),
        "expected a Type patch for Visibility::Visible"
    );

    // The Children relation, with the fixture's three child groups.
    let children = ast.get_children_ast(root);
    assert_eq!(children.len(), 3, "child count");

    // Walk the children and confirm the shape of several patches.
    let mut saw_scene_root_tuple = false;
    let mut saw_template = false;
    let mut saw_intensity_float = false;
    let mut saw_transform_translation_struct = false;

    for &child in &children {
        for patch in patches_of(&ast, child) {
            match patch {
                BsnPatch::TupleStruct(data) if data.type_path.ends_with("SceneRoot") => {
                    saw_scene_root_tuple = true;
                    assert_eq!(data.values.len(), 1);
                    assert!(matches!(data.values[0], BsnValue::String(_)));
                }
                BsnPatch::Template(tp, _) if tp.ends_with("CascadeShadowConfigBuilder") => {
                    saw_template = true;
                }
                BsnPatch::Struct(data) if data.type_path.ends_with("EnvironmentMapLight") => {
                    for field in &data.fields.0 {
                        if field.name == "intensity" {
                            assert!(
                                matches!(field.value, BsnValue::Float(f) if (f - 250.0).abs() < 1e-6),
                                "intensity should be a float of 250.0"
                            );
                            saw_intensity_float = true;
                        }
                    }
                }
                BsnPatch::Struct(data) if data.type_path.ends_with("Transform") => {
                    for field in &data.fields.0 {
                        if field.name == "translation" {
                            let BsnValue::Struct(inner) = &field.value else {
                                panic!("translation should be a nested struct");
                            };
                            assert!(
                                inner
                                    .fields
                                    .0
                                    .iter()
                                    .all(|f| matches!(f.value, BsnValue::Float(_))),
                                "translation components should be floats"
                            );
                            saw_transform_translation_struct = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    assert!(
        saw_scene_root_tuple,
        "expected a SceneRoot(...) tuple patch"
    );
    assert!(
        saw_template,
        "expected an @CascadeShadowConfigBuilder template patch"
    );
    assert!(saw_intensity_float, "expected an intensity float field");
    assert!(
        saw_transform_translation_struct,
        "expected a Transform translation struct field"
    );
}

#[test]
fn malformed_input_returns_error() {
    // Unbalanced brace: the loader should return an error, not panic.
    assert!(parse_bsn_text("Foo { a: ").is_err());
    // A stray delimiter that cannot begin a patch.
    assert!(parse_bsn_text("]").is_err());
}

#[test]
fn empty_input_yields_no_roots() {
    for text in ["", "   ", "\n\t\n", "// comment only\n"] {
        let ast = parse_bsn_text(text).expect("empty-ish input should load");
        assert!(
            ast.roots.is_empty(),
            "expected no roots for {text:?}, got {}",
            ast.roots.len()
        );
    }
}

#[test]
fn scene_load_routes_embedded_assets_and_resolves_references() {
    use bevy::app::{App, TaskPoolPlugin};
    use bevy::asset::{Asset, AssetApp, AssetPlugin, Assets, Handle, ReflectAsset, ReflectHandle};
    use bevy::ecs::prelude::*;
    use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
    use bevy::prelude::ReflectDefault;
    use bevy::reflect::Reflect;

    use jackdaw_bsn::load_bsn_scene;

    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct ProbeMaterial {
        shininess: f32,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct UsesMaterial {
        material: Handle<ProbeMaterial>,
    }

    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()));
    app.init_asset::<ProbeMaterial>();
    {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let mut w = registry.write();
        w.register::<ProbeMaterial>();
        w.register::<UsesMaterial>();
        w.register::<Handle<ProbeMaterial>>();
        w.register_type_data::<ProbeMaterial, ReflectAsset>();
        w.register_type_data::<Handle<ProbeMaterial>, ReflectHandle>();
    }
    app.world_mut().init_resource::<jackdaw_bsn::SceneBsnAst>();

    let text = r##"
bevy_ecs::hierarchy::Children [
    #Shiny
    scene_load_routes_embedded_assets_and_resolves_references::ProbeMaterial {
        shininess: 9.5,
    }
    ,
    #Thing
    scene_load_routes_embedded_assets_and_resolves_references::UsesMaterial {
        material: "#Shiny",
    }
]
"##;
    // The test types' reflected type paths are module-qualified; rewrite the
    // placeholder segment to each type's real reflect path.
    use bevy::reflect::TypePath;
    let text = text
        .replace(
            "scene_load_routes_embedded_assets_and_resolves_references::ProbeMaterial",
            ProbeMaterial::type_path(),
        )
        .replace(
            "scene_load_routes_embedded_assets_and_resolves_references::UsesMaterial",
            UsesMaterial::type_path(),
        );

    let loaded = load_bsn_scene(app.world_mut(), &text).expect("scene loads");

    // One asset entry landed in the store; one entity spawned (not two).
    assert_eq!(loaded.assets.len(), 1);
    assert_eq!(loaded.assets[0].name, "Shiny");
    assert_eq!(loaded.entities.len(), 1);

    let stored = app
        .world()
        .resource::<Assets<ProbeMaterial>>()
        .get(&loaded.assets[0].handle.clone().typed::<ProbeMaterial>())
        .expect("asset in store");
    assert!((stored.shininess - 9.5).abs() < 1e-6);

    // The spawned entity's handle resolves to that same asset.
    let entity = loaded.entities[0];
    let uses = app
        .world()
        .get::<UsesMaterial>(entity)
        .expect("component applied");
    assert_eq!(
        uses.material.id(),
        loaded.assets[0]
            .handle
            .clone()
            .typed::<ProbeMaterial>()
            .id(),
        "#Shiny reference must bind to the embedded asset"
    );
}

/// A scene may reference an embedded named asset by either spelling: the
/// scene-inline `#Name` or the catalog `@Name`. The loader records both, so
/// two components using one spelling each resolve to the same asset.
#[test]
fn scene_load_resolves_both_inline_and_catalog_reference_spellings() {
    use bevy::app::{App, TaskPoolPlugin};
    use bevy::asset::{Asset, AssetApp, AssetPlugin, Handle, ReflectAsset, ReflectHandle};
    use bevy::ecs::prelude::*;
    use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
    use bevy::prelude::ReflectDefault;
    use bevy::reflect::{Reflect, TypePath};

    use jackdaw_bsn::load_bsn_scene;

    #[derive(Asset, Reflect, Clone, Default)]
    #[reflect(Default)]
    struct SpellingMaterial {
        shininess: f32,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct SpellingUser {
        material: Handle<SpellingMaterial>,
    }

    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()));
    app.init_asset::<SpellingMaterial>();
    {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let mut w = registry.write();
        w.register::<SpellingMaterial>();
        w.register::<SpellingUser>();
        w.register::<Handle<SpellingMaterial>>();
        w.register_type_data::<SpellingMaterial, ReflectAsset>();
        w.register_type_data::<Handle<SpellingMaterial>, ReflectHandle>();
    }
    app.world_mut().init_resource::<jackdaw_bsn::SceneBsnAst>();

    let text = r##"
bevy_ecs::hierarchy::Children [
    #Shiny
    MATERIAL_TYPE {
        shininess: 2.5,
    }
    ,
    #InlineUser
    USER_TYPE {
        material: "#Shiny",
    }
    ,
    #CatalogUser
    USER_TYPE {
        material: "@Shiny",
    }
]
"##;
    let text = text
        .replace("MATERIAL_TYPE", SpellingMaterial::type_path())
        .replace("USER_TYPE", SpellingUser::type_path());

    let loaded = load_bsn_scene(app.world_mut(), &text).expect("scene loads");

    assert_eq!(loaded.assets.len(), 1);
    assert_eq!(loaded.entities.len(), 2, "one asset root, two entity roots");

    let asset_id = loaded.assets[0]
        .handle
        .clone()
        .typed::<SpellingMaterial>()
        .id();
    for &entity in &loaded.entities {
        let user = app
            .world()
            .get::<SpellingUser>(entity)
            .expect("component applied");
        assert_eq!(
            user.material.id(),
            asset_id,
            "both #Name and @Name spellings must bind to the embedded asset"
        );
    }
}

#[test]
fn stable_node_id_lookup_finds_nested_nodes() {
    use bevy::app::App;
    use bevy::ecs::reflect::AppTypeRegistry;

    use jackdaw_bsn::{SceneBsnAst, apply_dirty_ast_patches, parse_bsn_text, spawn_from_ast};

    let text = r##"
bevy_ecs::hierarchy::Children [
    #Parent
    jackdaw_scene_types::node_id::SceneNodeId(11)
    bevy_ecs::hierarchy::Children [
        #Child
        jackdaw_scene_types::node_id::SceneNodeId(22)
    ]
    ,
    #Sibling
    jackdaw_scene_types::node_id::SceneNodeId(33)
]
"##;

    let mut app = App::new();
    app.add_plugins(bevy::app::TaskPoolPlugin::default());
    app.world_mut().init_resource::<AppTypeRegistry>();
    let ast = parse_bsn_text(text).expect("parses");
    app.world_mut().insert_resource(ast);
    let spawned = spawn_from_ast(app.world_mut());
    apply_dirty_ast_patches(app.world_mut());
    assert_eq!(spawned.len(), 3);

    let ast = app.world().resource::<SceneBsnAst>();
    let child_node = ast.node_by_stable_id(22).expect("nested child found");
    assert_eq!(ast.get_name(child_node), Some("Child"));
    let sibling = ast.entity_for_stable_id(33).expect("sibling entity");
    assert_eq!(
        app.world()
            .get::<bevy::prelude::Name>(sibling)
            .unwrap()
            .as_str(),
        "Sibling"
    );
    assert!(ast.node_by_stable_id(999).is_none());
}
