//! Read-path test: `.bsn` text through parse, spawn, and reflect-apply into a
//! live ECS `World`.

use bevy::ecs::hierarchy::Children;
use bevy::ecs::name::Name;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::world::World;
use bevy::math::Vec3;
use bevy::transform::components::Transform;

use jackdaw_bsn::{SceneBsnAst, apply_dirty_ast_patches, parse_bsn_text, spawn_from_ast};

#[test]
fn bsn_text_materializes_into_ecs() {
    // A root entity with a name and a Transform (translation.x overridden),
    // plus one child with its own Transform.
    let text = "\
#Root
bevy_transform::components::transform::Transform {
    translation: glam::Vec3 { x: 5.0 },
}
bevy_ecs::hierarchy::Children [
    #Child
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 1.0 },
    }
]
";

    let mut world = World::new();

    // Register the reflectable component types the patches reference. Registering
    // Transform pulls in its field types (Vec3, f32) as dependencies.
    let registry = AppTypeRegistry::default();
    registry.write().register::<Transform>();
    world.insert_resource(registry);

    // Parse and install the AST as the scene resource.
    let ast = parse_bsn_text(text).expect("bsn should parse");
    world.insert_resource(ast);

    // Spawn ECS entities from the AST, then apply the patches via reflection.
    let spawned = spawn_from_ast(&mut world);
    apply_dirty_ast_patches(&mut world);

    assert_eq!(spawned.len(), 2, "one root plus one child spawned");

    // Locate the root by its name.
    let root = spawned
        .iter()
        .copied()
        .find(|&e| world.get::<Name>(e).is_some_and(|n| n.as_str() == "Root"))
        .expect("root entity with Name(\"Root\")");

    // The root's Transform component exists and the overridden scalar landed,
    // while unmentioned components of translation kept their default (0.0).
    let root_transform = world
        .get::<Transform>(root)
        .expect("root Transform component");
    assert!((root_transform.translation.x - 5.0).abs() < f32::EPSILON);
    assert!(root_transform.translation.y.abs() < f32::EPSILON);
    assert_eq!(root_transform.translation, Vec3::new(5.0, 0.0, 0.0));

    // The parent/child hierarchy is correct.
    let children = world.get::<Children>(root).expect("root Children");
    assert_eq!(children.len(), 1, "root has exactly one child");
    let child = children[0];

    let child_name = world.get::<Name>(child).map(|n| n.as_str().to_string());
    assert_eq!(child_name.as_deref(), Some("Child"), "child name");

    let child_transform = world
        .get::<Transform>(child)
        .expect("child Transform component");
    assert!((child_transform.translation.x - 1.0).abs() < f32::EPSILON);

    // Handle resolution (the `ReflectHandle` branch of `bsn_value_to_reflect`)
    // is exercised structurally by the code path; asserting a resolved
    // `Handle<T>` would require standing up a full asset backend, so this test
    // covers the non-handle component round-trip only.
    let _ = SceneBsnAst::default();
}

/// A component with a `HashMap<String, i32>` field, applied from a `map[...]`
/// literal. Proves the `DynamicMap` built by `map_value_to_reflect` converts
/// into a concrete `HashMap` when applied onto the component (the concrete
/// map's `FromReflect`-backed `insert_boxed` does the key/value conversion).
#[test]
fn bsn_map_literal_materializes_into_concrete_hashmap() {
    use bevy::ecs::reflect::ReflectComponent;
    use bevy::reflect::Reflect;
    use bevy::reflect::prelude::ReflectDefault;
    use std::collections::HashMap;

    #[derive(bevy::ecs::component::Component, Reflect, Default)]
    #[reflect(Default, Component)]
    struct HasMap {
        data: HashMap<String, i32>,
    }

    let mut world = World::new();
    let registry = AppTypeRegistry::default();
    registry.write().register::<HasMap>();

    let type_path = registry
        .read()
        .get(std::any::TypeId::of::<HasMap>())
        .expect("HasMap registered")
        .type_info()
        .type_path()
        .to_string();
    world.insert_resource(registry);

    let text = format!("{type_path} {{\n    data: map[(\"a\", 1), (\"b\", 2)],\n}}\n");

    let ast = parse_bsn_text(&text).expect("bsn should parse");
    world.insert_resource(ast);

    let spawned = spawn_from_ast(&mut world);
    apply_dirty_ast_patches(&mut world);
    assert_eq!(spawned.len(), 1);

    let comp = world
        .get::<HasMap>(spawned[0])
        .expect("HasMap component applied");
    assert_eq!(comp.data.len(), 2, "both map entries applied");
    assert_eq!(comp.data.get("a"), Some(&1));
    assert_eq!(comp.data.get("b"), Some(&2));
}

/// A multiline `map[...]` whose values are nested struct instances. Proves the
/// map apply path recurses through `bsn_value_to_reflect` for non-scalar
/// values, not just primitives, and materializes a concrete `HashMap` of
/// structs.
#[test]
fn bsn_map_with_struct_values_materializes() {
    use bevy::ecs::reflect::ReflectComponent;
    use bevy::reflect::Reflect;
    use bevy::reflect::prelude::ReflectDefault;
    use std::collections::HashMap;

    #[derive(Reflect, Default, PartialEq, Debug)]
    #[reflect(Default)]
    struct Slot {
        amount: i32,
        enabled: bool,
    }

    #[derive(bevy::ecs::component::Component, Reflect, Default)]
    #[reflect(Default, Component)]
    struct Props {
        data: HashMap<String, Slot>,
    }

    let mut world = World::new();
    let registry = AppTypeRegistry::default();
    {
        let mut w = registry.write();
        w.register::<Props>();
        w.register::<Slot>();
    }

    let (props_path, slot_path) = {
        let r = registry.read();
        let props = r
            .get(std::any::TypeId::of::<Props>())
            .expect("Props registered")
            .type_info()
            .type_path()
            .to_string();
        let slot = r
            .get(std::any::TypeId::of::<Slot>())
            .expect("Slot registered")
            .type_info()
            .type_path()
            .to_string();
        (props, slot)
    };
    world.insert_resource(registry);

    let text = format!(
        "{props_path} {{\n    data: map[\n        (\"one\", {slot_path} {{ amount: 5, enabled: true }}),\n        (\"two\", {slot_path} {{ amount: 9, enabled: false }}),\n    ],\n}}\n"
    );

    let ast = parse_bsn_text(&text).expect("bsn should parse");
    world.insert_resource(ast);

    let spawned = spawn_from_ast(&mut world);
    apply_dirty_ast_patches(&mut world);

    let comp = world
        .get::<Props>(spawned[0])
        .expect("Props component applied");
    assert_eq!(comp.data.len(), 2);
    assert_eq!(
        comp.data.get("one"),
        Some(&Slot {
            amount: 5,
            enabled: true
        })
    );
    assert_eq!(
        comp.data.get("two"),
        Some(&Slot {
            amount: 9,
            enabled: false
        })
    );
}
