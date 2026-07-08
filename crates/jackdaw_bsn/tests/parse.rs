//! Parser tests for the `.bsn` front-end.

use jackdaw_bsn::parse::{BsnPatch, BsnPatches, BsnRoot};
use jackdaw_bsn::{BsnExpr, BsnNamedTuple, BsnStruct, BsnVar, parse_bsn};

use bevy::ecs::entity::Entity;
use jackdaw_bsn::BsnAst;

/// Fetch the root list of patch entities from a parsed AST.
fn root_patches(ast: &BsnAst) -> Vec<Entity> {
    let root = ast.0.resource::<BsnRoot>().0;
    ast.0
        .get::<BsnPatches>(root)
        .expect("root BsnPatches")
        .0
        .clone()
}

fn patch(ast: &BsnAst, id: Entity) -> &BsnPatch {
    ast.0.get::<BsnPatch>(id).expect("BsnPatch node")
}

fn expr(ast: &BsnAst, id: Entity) -> &BsnExpr {
    ast.0.get::<BsnExpr>(id).expect("BsnExpr node")
}

#[test]
fn parses_example_fixture() {
    let text = include_str!("fixtures/example.bsn");
    let ast = parse_bsn(text).expect("example.bsn should parse");

    let patches = root_patches(&ast);
    // #Root, Transform, Visibility::Visible, Children [...]
    assert_eq!(patches.len(), 4, "top-level patch count");

    // First patch is the `#Root` name.
    match patch(&ast, patches[0]) {
        BsnPatch::Name(name, _index) => assert_eq!(name, "Root"),
        _ => panic!("expected a Name patch first"),
    }

    // A bare type-path Var patch: Transform.
    match patch(&ast, patches[1]) {
        BsnPatch::Var(BsnVar(symbol, is_template)) => {
            assert!(!is_template);
            assert_eq!(symbol.1, "Transform");
        }
        _ => panic!("expected a Var patch for Transform"),
    }

    // An enum unit-variant patch: Visibility::Visible.
    match patch(&ast, patches[2]) {
        BsnPatch::Var(BsnVar(symbol, _)) => {
            assert_eq!(symbol.1, "Visible");
            assert!(symbol.0.iter().any(|seg| seg == "Visibility"));
        }
        _ => panic!("expected a Var patch for Visibility::Visible"),
    }

    // The Children relation and its grouped child patches.
    let BsnPatch::Relation(relation) = patch(&ast, patches[3]) else {
        panic!("expected a Relation patch for Children");
    };
    assert_eq!(relation.0.1, "Children");
    // Groups are comma-separated; the fixture has three groups.
    assert_eq!(relation.1.len(), 3, "Children group count");

    // First group bundles four patches onto one child.
    let group0 = ast
        .0
        .get::<BsnPatches>(relation.1[0])
        .expect("group patches");
    assert_eq!(group0.0.len(), 4);

    // A struct patch with a known field lives in the first group.
    let mut saw_struct_field = false;
    for &child in &group0.0 {
        if let BsnPatch::Struct(BsnStruct(_symbol, fields, _)) = patch(&ast, child)
            && fields.iter().any(|f| f.0 == "intensity")
        {
            saw_struct_field = true;
        }
    }
    assert!(
        saw_struct_field,
        "expected a struct patch with an `intensity` field"
    );

    // A tuple patch: SceneRoot("...").
    let mut saw_tuple = false;
    // A template patch: @CascadeShadowConfigBuilder.
    let mut saw_template = false;
    for &group_id in &relation.1 {
        let group = ast.0.get::<BsnPatches>(group_id).expect("group patches");
        for &child in &group.0 {
            match patch(&ast, child) {
                BsnPatch::NamedTuple(BsnNamedTuple(symbol, args, _)) if symbol.1 == "SceneRoot" => {
                    saw_tuple = true;
                    assert_eq!(args.len(), 1);
                    assert!(matches!(expr(&ast, args[0]), BsnExpr::StringLit(_)));
                }
                BsnPatch::Struct(BsnStruct(symbol, _, is_template))
                    if symbol.1 == "CascadeShadowConfigBuilder" =>
                {
                    assert!(*is_template, "template patch should carry the @ marker");
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
    let ast = parse_bsn("foo::bar::Baz").unwrap();
    let patches = root_patches(&ast);
    assert_eq!(patches.len(), 1);
    let BsnPatch::Var(BsnVar(symbol, is_template)) = patch(&ast, patches[0]) else {
        panic!("expected Var");
    };
    assert!(!is_template);
    assert_eq!(symbol.0, vec!["foo".to_string(), "bar".to_string()]);
    assert_eq!(symbol.1, "Baz");
}

#[test]
fn parses_struct_with_fields() {
    let ast = parse_bsn("Thing { a: 1, b: 2.5 }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(symbol, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(symbol.1, "Thing");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].0, "a");
    assert!(matches!(expr(&ast, fields[0].1), BsnExpr::IntLit(1)));
    assert!(matches!(expr(&ast, fields[1].1), BsnExpr::FloatLit(_)));
}

#[test]
fn parses_tuple() {
    let ast = parse_bsn(r#"Wrap("hello", true)"#).unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::NamedTuple(BsnNamedTuple(symbol, args, _)) = patch(&ast, patches[0]) else {
        panic!("expected NamedTuple");
    };
    assert_eq!(symbol.1, "Wrap");
    assert_eq!(args.len(), 2);
    assert!(matches!(expr(&ast, args[0]), BsnExpr::StringLit(s) if s == "hello"));
    assert!(matches!(expr(&ast, args[1]), BsnExpr::BoolLit(true)));
}

#[test]
fn parses_list_literal() {
    let ast = parse_bsn("Holder { items: [1, 2, 3] }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnExpr::List(items) = expr(&ast, fields[0].1) else {
        panic!("expected List expr");
    };
    assert_eq!(items.len(), 3);
}

#[test]
fn parses_map_literal() {
    let ast = parse_bsn(r#"Comp { data: map[("a", 1), ("b", 2)] }"#).unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnExpr::Map(entries) = expr(&ast, fields[0].1) else {
        panic!("expected Map expr");
    };
    assert_eq!(entries.len(), 2);
    // First entry: ("a", 1).
    let (k0, v0) = entries[0];
    assert!(matches!(expr(&ast, k0), BsnExpr::StringLit(s) if s == "a"));
    assert!(matches!(expr(&ast, v0), BsnExpr::IntLit(1)));
    // Second entry: ("b", 2).
    let (k1, v1) = entries[1];
    assert!(matches!(expr(&ast, k1), BsnExpr::StringLit(s) if s == "b"));
    assert!(matches!(expr(&ast, v1), BsnExpr::IntLit(2)));
}

#[test]
fn parses_empty_map_literal() {
    let ast = parse_bsn("Comp { data: map[] }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnExpr::Map(entries) = expr(&ast, fields[0].1) else {
        panic!("expected Map expr");
    };
    assert!(entries.is_empty());
}

#[test]
fn map_is_a_contextual_keyword() {
    // `map` is only special immediately before `[`. As a field name, component
    // name, or path segment it remains an ordinary identifier.
    let ast = parse_bsn("Comp { map: 1 }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(fields[0].0, "map");

    let ast = parse_bsn("map { x: 1 }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(symbol, _, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct named map");
    };
    assert_eq!(symbol.1, "map");
}

#[test]
fn parses_empty_list_literal() {
    let ast = parse_bsn("Holder { items: [] }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    let BsnExpr::List(items) = expr(&ast, fields[0].1) else {
        panic!("expected List expr");
    };
    assert!(items.is_empty());
}

#[test]
fn parses_base_inherit() {
    let ast = parse_bsn(r#":"base.bsn""#).unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Base(path) = patch(&ast, patches[0]) else {
        panic!("expected Base");
    };
    assert_eq!(path, "base.bsn");
}

#[test]
fn parses_name_with_spaces() {
    let ast = parse_bsn(r#"#"a name with spaces""#).unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Name(name, _) = patch(&ast, patches[0]) else {
        panic!("expected Name");
    };
    assert_eq!(name, "a name with spaces");
}

#[test]
fn parses_template_marker() {
    let ast = parse_bsn("@Foo { x: 1 }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(symbol, _, is_template)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(symbol.1, "Foo");
    assert!(is_template);
}

#[test]
fn ignores_comments() {
    let text = "\
// line comment
Foo /* block /* nested */ comment */ { a: 1 }
// trailing
";
    let ast = parse_bsn(text).unwrap();
    let patches = root_patches(&ast);
    assert_eq!(patches.len(), 1);
    assert!(matches!(patch(&ast, patches[0]), BsnPatch::Struct(_)));
}

#[test]
fn allows_trailing_commas() {
    let ast = parse_bsn("Foo { a: 1, b: 2, }").unwrap();
    let patches = root_patches(&ast);
    let BsnPatch::Struct(BsnStruct(_, fields, _)) = patch(&ast, patches[0]) else {
        panic!("expected Struct");
    };
    assert_eq!(fields.len(), 2);
}

#[test]
fn malformed_input_errors_without_panic() {
    // Unbalanced brace: the grammar should reject this.
    assert!(parse_bsn("Foo { a: ").is_err());
    // A stray delimiter that cannot begin a patch.
    assert!(parse_bsn("]").is_err());
}
