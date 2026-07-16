//! Parser tests for the `.bsn` front-end. The parser builds the editor
//! document directly, so assertions inspect [`SceneBsnAst`] patches and owned
//! [`BsnValue`] trees.

use bevy::ecs::entity::Entity;
use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst, parse_bsn};

/// Parse text and return the document plus the top-level patch group's
/// patch entities.
fn parse(text: &str) -> (SceneBsnAst, Vec<Entity>) {
    let (ast, root) = parse_bsn(text).expect("text should parse");
    let patches = ast.get_patches(root).expect("root patch group").0.clone();
    (ast, patches)
}

fn patch(ast: &SceneBsnAst, id: Entity) -> &BsnPatch {
    ast.get_patch(id).expect("patch node")
}

#[test]
fn parses_example_fixture() {
    let text = include_str!("fixtures/example.bsn");
    let (ast, patches) = parse(text);
    // #Root, Transform, Visibility::Visible, Children [...]
    assert_eq!(patches.len(), 4, "top-level patch count");

    // First patch is the `#Root` name.
    match patch(&ast, patches[0]) {
        BsnPatch::Name(name) => assert_eq!(name, "Root"),
        _ => panic!("expected a Name patch first"),
    }

    // A bare type-path patch: Transform.
    match patch(&ast, patches[1]) {
        BsnPatch::Type(path) => assert!(path.ends_with("Transform"), "got {path}"),
        _ => panic!("expected a Type patch for Transform"),
    }

    // An enum unit-variant patch: Visibility::Visible.
    match patch(&ast, patches[2]) {
        BsnPatch::Type(path) => {
            assert!(path.ends_with("::Visible"));
            assert!(path.contains("Visibility"));
        }
        _ => panic!("expected a Type patch for Visibility::Visible"),
    }

    // The Children relation and its grouped child nodes.
    let BsnPatch::Children(children) = patch(&ast, patches[3]) else {
        panic!("expected a Children patch");
    };
    // Groups are comma-separated; the fixture has three groups.
    assert_eq!(children.len(), 3, "Children group count");

    // First group bundles four patches onto one child.
    let group0 = ast.get_patches(children[0]).expect("group patches");
    assert_eq!(group0.0.len(), 4);

    // A struct patch with a known field lives in the first group.
    let saw_struct_field = group0.0.iter().any(|&child| {
        matches!(
            patch(&ast, child),
            BsnPatch::Struct(data) if data.fields.0.iter().any(|f| f.name == "intensity")
        )
    });
    assert!(
        saw_struct_field,
        "expected a struct patch with an `intensity` field"
    );

    // A tuple patch: SceneRoot("..."), and a template patch:
    // @CascadeShadowConfigBuilder.
    let mut saw_tuple = false;
    let mut saw_template = false;
    for &group_id in children {
        let group = ast.get_patches(group_id).expect("group patches");
        for &child in &group.0 {
            match patch(&ast, child) {
                BsnPatch::TupleStruct(data) if data.type_path.ends_with("SceneRoot") => {
                    saw_tuple = true;
                    assert_eq!(data.values.len(), 1);
                    assert!(matches!(&data.values[0], BsnValue::String(_)));
                }
                BsnPatch::Template(path, _) if path.ends_with("CascadeShadowConfigBuilder") => {
                    saw_template = true;
                }
                _ => {}
            }
        }
    }
    assert!(saw_tuple, "expected a SceneRoot(...) tuple patch");
    assert!(
        saw_template,
        "expected an @CascadeShadowConfigBuilder template patch"
    );
}

#[test]
fn parses_bare_type_path() {
    let (ast, patches) = parse("foo::bar::Baz");
    assert_eq!(patches.len(), 1);
    let BsnPatch::Type(path) = patch(&ast, patches[0]) else {
        panic!("expected Type");
    };
    assert_eq!(path, "foo::bar::Baz");
}

#[test]
fn parses_struct_with_fields() {
    let (ast, patches) = parse("Thing { a: 1, b: 2.5 }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(data.type_path, "Thing");
    assert_eq!(data.fields.0.len(), 2);
    assert_eq!(data.fields.0[0].name, "a");
    assert!(matches!(data.fields.0[0].value, BsnValue::Int(1)));
    assert!(matches!(data.fields.0[1].value, BsnValue::Float(_)));
}

#[test]
fn parses_tuple() {
    let (ast, patches) = parse(r#"Wrap("hello", true)"#);
    let BsnPatch::TupleStruct(data) = patch(&ast, patches[0]) else {
        panic!("expected TupleStruct");
    };
    assert_eq!(data.type_path, "Wrap");
    assert_eq!(data.values.len(), 2);
    assert!(matches!(&data.values[0], BsnValue::String(s) if s == "hello"));
    assert!(matches!(data.values[1], BsnValue::Bool(true)));
}

#[test]
fn parses_list_literal() {
    let (ast, patches) = parse("Holder { items: [1, 2, 3] }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnValue::List(items) = &data.fields.0[0].value else {
        panic!("expected List value");
    };
    assert_eq!(items.len(), 3);
}

#[test]
fn parses_map_literal() {
    let (ast, patches) = parse(r#"Comp { data: map[("a", 1), ("b", 2)] }"#);
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnValue::Map(entries) = &data.fields.0[0].value else {
        panic!("expected Map value");
    };
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0].0, BsnValue::String(s) if s == "a"));
    assert!(matches!(entries[0].1, BsnValue::Int(1)));
    assert!(matches!(&entries[1].0, BsnValue::String(s) if s == "b"));
    assert!(matches!(entries[1].1, BsnValue::Int(2)));
}

#[test]
fn parses_empty_map_literal() {
    let (ast, patches) = parse("Comp { data: map[] }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnValue::Map(entries) = &data.fields.0[0].value else {
        panic!("expected Map value");
    };
    assert!(entries.is_empty());
}

#[test]
fn map_is_a_contextual_keyword() {
    // `map` is only special immediately before `[`. As a field name, component
    // name, or path segment it remains an ordinary identifier.
    let (ast, patches) = parse("Comp { map: 1 }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(data.fields.0[0].name, "map");

    let (ast, patches) = parse("map { x: 1 }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct named map");
    };
    assert_eq!(data.type_path, "map");
}

#[test]
fn parses_empty_list_literal() {
    let (ast, patches) = parse("Holder { items: [] }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnValue::List(items) = &data.fields.0[0].value else {
        panic!("expected List value");
    };
    assert!(items.is_empty());
}

#[test]
fn parses_base_inherit() {
    let (ast, patches) = parse(r#":"base.bsn""#);
    let BsnPatch::Base(path) = patch(&ast, patches[0]) else {
        panic!("expected Base");
    };
    assert_eq!(path, "base.bsn");
}

#[test]
fn parses_name_with_spaces() {
    let (ast, patches) = parse(r#"#"a name with spaces""#);
    let BsnPatch::Name(name) = patch(&ast, patches[0]) else {
        panic!("expected Name");
    };
    assert_eq!(name, "a name with spaces");
}

#[test]
fn parses_template_marker() {
    let (ast, patches) = parse("@Foo { x: 1 }");
    let BsnPatch::Template(path, fields) = patch(&ast, patches[0]) else {
        panic!("expected Template");
    };
    assert_eq!(path, "Foo");
    assert!(fields.is_some());
}

#[test]
fn ignores_comments() {
    let text = "\
// line comment
Foo /* block /* nested */ comment */ { a: 1 }
// trailing
";
    let (ast, patches) = parse(text);
    assert_eq!(patches.len(), 1);
    assert!(matches!(patch(&ast, patches[0]), BsnPatch::Struct(_)));
}

#[test]
fn allows_trailing_commas() {
    let (ast, patches) = parse("Foo { a: 1, b: 2, }");
    let BsnPatch::Struct(data) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(data.fields.0.len(), 2);
}

#[test]
fn malformed_input_errors_without_panic() {
    // Unbalanced brace: the grammar should reject this.
    assert!(parse_bsn("Foo { a: ").is_err());
    // A stray delimiter that cannot begin a patch.
    assert!(parse_bsn("]").is_err());
}

#[test]
fn unterminated_containers_error_without_panic() {
    // A map literal whose bracket never closes.
    assert!(parse_bsn(r#"Comp { data: map[("a", 1) }"#).is_err());
    // A struct body whose brace never closes.
    assert!(parse_bsn("Comp { a: 1").is_err());
    // A list literal whose bracket never closes.
    assert!(parse_bsn("Holder { items: [1, 2 }").is_err());
}

#[test]
fn malformed_type_paths_error_without_panic() {
    // A path separator with nothing after it.
    assert!(parse_bsn("foo::").is_err());
    // A doubled path separator inside a variant path.
    assert!(parse_bsn("Visibility::::Visible").is_err());
}

#[test]
fn malformed_number_literals_error_without_panic() {
    // Two decimal points in one literal.
    assert!(parse_bsn("Foo { a: 1.2.3 }").is_err());
    // Letters glued onto a number where a value is expected.
    assert!(parse_bsn("Foo { a: 12abc }").is_err());
}

/// A struct body that repeats a field name is rejected: applying it would
/// silently let the last occurrence win, hiding hand-edit mistakes.
#[test]
fn duplicate_field_names_are_rejected() {
    let message = match parse_bsn("Foo { a: 1, a: 2 }") {
        Err(err) => err.to_string(),
        Ok(_) => panic!("duplicate fields must be rejected"),
    };
    assert!(
        message.contains("duplicate field 'a'"),
        "error names the duplicated field: {message}"
    );

    // Nested struct values are checked too.
    assert!(parse_bsn("Foo { t: Bar { x: 1, x: 2 } }").is_err());
}
