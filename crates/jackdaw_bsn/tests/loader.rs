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
                                inner.fields.0.iter().all(|f| matches!(f.value, BsnValue::Float(_))),
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

    assert!(saw_scene_root_tuple, "expected a SceneRoot(...) tuple patch");
    assert!(saw_template, "expected an @CascadeShadowConfigBuilder template patch");
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
