//! Read-path test: `.bsn` text through parse, spawn, and reflect-apply into a
//! live ECS `World`.

use bevy::ecs::hierarchy::Children;
use bevy::ecs::name::Name;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::world::World;
use bevy::math::Vec3;
use bevy::transform::components::Transform;

use jackdaw_bsn::{apply_dirty_ast_patches, parse_bsn_text, spawn_from_ast, SceneBsnAst};

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
        .find(|&e| {
            world
                .get::<Name>(e)
                .is_some_and(|n| n.as_str() == "Root")
        })
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
