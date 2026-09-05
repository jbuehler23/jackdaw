//! Path-navigation edge cases for `set_bsn_field`, beyond the parity matrix
//! in `apply.rs`: deep dotted paths that mint intermediate structs, list
//! element writes, and the no-op behavior of paths the navigator does not
//! resolve (out-of-range indices, absent map keys, malformed brackets).

use bevy::ecs::prelude::Component;
use bevy::prelude::ReflectDefault;
use bevy::reflect::{Reflect, TypePath, TypeRegistry};

use jackdaw_bsn::{
    BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnValue, SceneBsnAst,
    component_to_bsn_patch, get_bsn_field, set_bsn_field,
};

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Default)]
struct Inner {
    v: f32,
}

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Default)]
struct Mid {
    inner: Inner,
}

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Default)]
struct Outer {
    mid: Mid,
}

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Default)]
struct Holder {
    items: Vec<f32>,
    props: std::collections::HashMap<String, f32>,
}

fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::new();
    registry.register::<Inner>();
    registry.register::<Mid>();
    registry.register::<Outer>();
    registry.register::<Holder>();
    registry.register::<Vec<f32>>();
    registry.register::<std::collections::HashMap<String, f32>>();
    registry.register::<String>();
    registry.register::<f32>();
    registry
}

/// One document node carrying the given patch, ready for field edits.
fn one_patch_ast(patch: BsnPatch) -> (SceneBsnAst, bevy::ecs::entity::Entity) {
    let mut ast = SceneBsnAst::default();
    let patch_entity = ast.world.spawn(patch).id();
    let patches_entity = ast.world.spawn(BsnPatches(vec![patch_entity])).id();
    (ast, patches_entity)
}

fn empty_struct_patch(type_path: &str) -> BsnPatch {
    BsnPatch::Struct(BsnStructData {
        type_path: type_path.to_string(),
        fields: BsnStructFields::default(),
    })
}

/// A three-segment dotted write onto an empty struct patch creates each
/// intermediate struct on demand, typed from the registry.
#[test]
fn deep_dotted_write_creates_intermediate_structs() {
    let registry = registry();
    let tp = Outer::type_path();
    let (mut ast, node) = one_patch_ast(empty_struct_patch(tp));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "mid.inner.v",
        BsnValue::Float(4.5),
        &registry,
    );

    let leaf = get_bsn_field(&ast, node, tp, "mid.inner.v");
    assert_eq!(leaf, Some(BsnValue::Float(4.5)));

    // The minted intermediates carry their registry-derived type paths.
    let mid = get_bsn_field(&ast, node, tp, "mid");
    match mid {
        Some(BsnValue::Struct(data)) => assert_eq!(data.type_path, Mid::type_path()),
        other => panic!("expected the mid intermediate to be a struct, got {other:?}"),
    }
    let inner = get_bsn_field(&ast, node, tp, "mid.inner");
    match inner {
        Some(BsnValue::Struct(data)) => assert_eq!(data.type_path, Inner::type_path()),
        other => panic!("expected the inner intermediate to be a struct, got {other:?}"),
    }
}

/// Writing through an existing scalar at an intermediate segment replaces the
/// scalar with a struct so the deeper write can land.
#[test]
fn dotted_write_replaces_scalar_intermediate_with_struct() {
    let registry = registry();
    let tp = Outer::type_path();
    let (mut ast, node) = one_patch_ast(BsnPatch::Struct(BsnStructData {
        type_path: tp.to_string(),
        fields: BsnStructFields(vec![jackdaw_bsn::BsnField {
            name: "mid".to_string(),
            value: BsnValue::Float(0.0),
        }]),
    }));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "mid.inner.v",
        BsnValue::Float(2.0),
        &registry,
    );

    assert_eq!(
        get_bsn_field(&ast, node, tp, "mid.inner.v"),
        Some(BsnValue::Float(2.0))
    );
}

fn holder_ast(registry: &TypeRegistry) -> (SceneBsnAst, bevy::ecs::entity::Entity) {
    let mut props = std::collections::HashMap::new();
    props.insert("hp".to_string(), 10.0_f32);
    let holder = Holder {
        items: vec![10.0, 20.0, 30.0],
        props,
    };
    one_patch_ast(component_to_bsn_patch(&holder, registry))
}

/// A bracketed list index writes an existing element in place.
#[test]
fn bracket_index_writes_list_element() {
    let registry = registry();
    let tp = Holder::type_path();
    let (mut ast, node) = holder_ast(&registry);

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "items[1]",
        BsnValue::Float(99.0),
        &registry,
    );

    assert_eq!(
        get_bsn_field(&ast, node, tp, "items"),
        Some(BsnValue::List(vec![
            BsnValue::Float(10.0),
            BsnValue::Float(99.0),
            BsnValue::Float(30.0),
        ]))
    );
}

/// An out-of-range list index is a no-op: elements are only navigated when
/// they already exist, never appended.
#[test]
fn bracket_index_out_of_range_is_a_noop() {
    let registry = registry();
    let tp = Holder::type_path();
    let (mut ast, node) = holder_ast(&registry);
    let before = get_bsn_field(&ast, node, tp, "items");

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "items[9]",
        BsnValue::Float(99.0),
        &registry,
    );

    assert_eq!(get_bsn_field(&ast, node, tp, "items"), before);
}

/// Writing to a map key that is not present is a no-op: unlike struct fields,
/// map entries are never created on demand.
#[test]
fn bracket_key_absent_from_map_is_a_noop() {
    let registry = registry();
    let tp = Holder::type_path();
    let (mut ast, node) = holder_ast(&registry);

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "props[missing]",
        BsnValue::Float(1.0),
        &registry,
    );

    assert!(get_bsn_field(&ast, node, tp, "props[missing]").is_none());
    assert_eq!(
        get_bsn_field(&ast, node, tp, "props[hp]"),
        Some(BsnValue::Float(10.0))
    );
}

/// A bracket segment without its closing bracket resolves nothing: the write
/// is dropped and the read returns `None`.
#[test]
fn unterminated_bracket_segment_is_a_noop() {
    let registry = registry();
    let tp = Holder::type_path();
    let (mut ast, node) = holder_ast(&registry);
    let before = get_bsn_field(&ast, node, tp, "items");

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "items[1",
        BsnValue::Float(99.0),
        &registry,
    );

    assert!(get_bsn_field(&ast, node, tp, "items[1").is_none());
    assert_eq!(get_bsn_field(&ast, node, tp, "items"), before);
}

/// A dotted path continuing past a list element writes into that element.
#[test]
fn bracket_index_then_field_writes_into_nested_struct_element() {
    let registry = registry();

    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Default)]
    struct Deck {
        cards: Vec<Inner>,
    }
    let mut registry = registry;
    registry.register::<Deck>();
    registry.register::<Vec<Inner>>();

    let deck = Deck {
        cards: vec![Inner { v: 1.0 }, Inner { v: 2.0 }],
    };
    let tp = Deck::type_path();
    let (mut ast, node) = one_patch_ast(component_to_bsn_patch(&deck, &registry));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "cards[1].v",
        BsnValue::Float(7.0),
        &registry,
    );

    assert_eq!(
        get_bsn_field(&ast, node, tp, "cards[1].v"),
        Some(BsnValue::Float(7.0))
    );
    assert_eq!(
        get_bsn_field(&ast, node, tp, "cards[0].v"),
        Some(BsnValue::Float(1.0)),
        "the untouched element keeps its value"
    );
}

/// The first field write on a node with no patch for the type creates the
/// struct patch; the node's other patches are untouched.
#[test]
fn write_creates_struct_patch_when_type_absent() {
    let registry = registry();
    let tp = Inner::type_path();

    let mut ast = SceneBsnAst::default();
    let node = ast.create_entity_node(vec![BsnPatch::Name("Node".to_string())]);

    set_bsn_field(&mut ast, node, tp, "v", BsnValue::Float(3.0), &registry);

    assert_eq!(
        get_bsn_field(&ast, node, tp, "v"),
        Some(BsnValue::Float(3.0))
    );
    assert_eq!(ast.get_name(node), Some("Node"));
    assert_eq!(ast.component_type_paths(node), vec![tp.to_string()]);
}
