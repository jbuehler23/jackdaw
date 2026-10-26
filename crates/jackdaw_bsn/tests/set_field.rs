//! Path-navigation edge cases for `set_bsn_field`, beyond the parity matrix
//! in `apply.rs`: deep dotted paths that mint intermediate structs, list
//! element writes, and the no-op behavior of paths the navigator does not
//! resolve (out-of-range indices, absent map keys, malformed brackets).

use bevy::ecs::prelude::Component;
use bevy::ecs::reflect::ReflectComponent;
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

/// A field shaped like `TextFont::font_size`: the holder defaults to a variant
/// other than the enum's own default, so only the holder's default is the
/// value the document leaves out.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
enum Size {
    Px(f32),
    Rem(f32),
}

impl Default for Size {
    fn default() -> Self {
        Size::Px(20.0)
    }
}

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Label {
    size: Size,
    weight: f32,
}

impl Default for Label {
    fn default() -> Self {
        Label {
            size: Size::Rem(1.0),
            weight: 400.0,
        }
    }
}

/// A field shaped like `DistanceFog::falloff`: a struct variant.
#[derive(Reflect, Clone, Debug, PartialEq)]
enum Falloff {
    Linear { start: f32, end: f32 },
    Exponential { density: f32 },
}

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Fog {
    falloff: Falloff,
}

impl Default for Fog {
    fn default() -> Self {
        Fog {
            falloff: Falloff::Linear {
                start: 0.0,
                end: 100.0,
            },
        }
    }
}

#[derive(Component, Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Span {
    range: (f32, f32),
}

/// A component with nothing to take a missing value from.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
struct Undefaulted {
    size: Size,
}

fn shape_registry() -> TypeRegistry {
    let mut registry = TypeRegistry::new();
    registry.register::<Label>();
    registry.register::<Fog>();
    registry.register::<Span>();
    registry.register::<Undefaulted>();
    registry
}

/// The `T` the node's document gives back after a save and a reload: emitted
/// to text, read back, and applied onto a fresh entity.
fn reloaded<T: Component + Clone + TypePath>(
    ast: &SceneBsnAst,
    node: bevy::ecs::entity::Entity,
    registry: TypeRegistry,
) -> T {
    let text = jackdaw_bsn::emit_entity(ast, node);
    let (read, root) = jackdaw_bsn::parse_bsn(&text)
        .unwrap_or_else(|err| panic!("the emitted text reads back: {err}\n{text}"));
    let patch = read
        .find_patch_by_type_path(root, T::type_path())
        .and_then(|patch| read.get_patch(patch))
        .cloned()
        .unwrap_or_else(|| panic!("the emitted text holds the component:\n{text}"));
    let mut world = bevy::ecs::world::World::new();
    let app_registry = bevy::ecs::reflect::AppTypeRegistry::default();
    *app_registry.write() = registry;
    world.insert_resource(app_registry);
    let entity = world.spawn_empty().id();
    jackdaw_bsn::apply_component_patch(&mut world, entity, &patch);
    world
        .get::<T>(entity)
        .cloned()
        .unwrap_or_else(|| panic!("the component applies from:\n{text}"))
}

#[test]
fn an_edit_inside_an_enum_left_at_its_default_writes_the_whole_variant() {
    let registry = shape_registry();
    let tp = Label::type_path();
    let (mut ast, node) = one_patch_ast(BsnPatch::Type(tp.to_string()));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "size.0",
        BsnValue::Float(1.5),
        &registry,
    );

    assert!(
        matches!(
            get_bsn_field(&ast, node, tp, "size"),
            Some(BsnValue::TupleStruct(data)) if data.type_path.ends_with("Size::Rem")
        ),
        "the variant the holder defaults to is written whole, not the enum's own default",
    );
    assert_eq!(
        reloaded::<Label>(&ast, node, shape_registry()),
        Label {
            size: Size::Rem(1.5),
            weight: 400.0
        }
    );
}

#[test]
fn an_edit_inside_a_struct_variant_left_at_its_default_keeps_its_other_fields() {
    let registry = shape_registry();
    let tp = Fog::type_path();
    let (mut ast, node) = one_patch_ast(empty_struct_patch(tp));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "falloff.start",
        BsnValue::Float(5.0),
        &registry,
    );

    assert_eq!(
        reloaded::<Fog>(&ast, node, shape_registry()).falloff,
        Falloff::Linear {
            start: 5.0,
            end: 100.0
        }
    );
}

#[test]
fn an_edit_inside_a_tuple_left_at_its_default_writes_the_whole_tuple() {
    let registry = shape_registry();
    let tp = Span::type_path();
    let (mut ast, node) = one_patch_ast(empty_struct_patch(tp));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "range.1",
        BsnValue::Float(4.0),
        &registry,
    );

    assert_eq!(
        reloaded::<Span>(&ast, node, shape_registry()).range,
        (0.0, 4.0)
    );
}

#[test]
fn an_edit_inside_a_variant_the_document_already_holds_writes_only_that_part() {
    let registry = shape_registry();
    let tp = Label::type_path();
    let (mut ast, node) = one_patch_ast(component_to_bsn_patch(
        &Label {
            size: Size::Px(12.0),
            weight: 400.0,
        },
        &registry,
    ));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "size.0",
        BsnValue::Float(14.0),
        &registry,
    );

    assert_eq!(
        reloaded::<Label>(&ast, node, shape_registry()).size,
        Size::Px(14.0)
    );
}

#[test]
fn a_current_value_seeds_the_variant_instead_of_the_default() {
    let registry = shape_registry();
    let tp = Label::type_path();
    let (mut ast, node) = one_patch_ast(BsnPatch::Type(tp.to_string()));
    let live = Label {
        size: Size::Px(30.0),
        weight: 400.0,
    };

    jackdaw_bsn::try_set_bsn_field(
        &mut ast,
        node,
        tp,
        "size.0",
        BsnValue::Float(32.0),
        &registry,
        Some(&live),
    )
    .expect("the live value seeds the variant");

    assert_eq!(
        reloaded::<Label>(&ast, node, shape_registry()).size,
        Size::Px(32.0)
    );
}

#[test]
fn an_edit_with_nothing_to_take_the_variant_from_is_refused() {
    let registry = shape_registry();
    let tp = Undefaulted::type_path();
    let (mut ast, node) = one_patch_ast(empty_struct_patch(tp));

    let written = jackdaw_bsn::try_set_bsn_field(
        &mut ast,
        node,
        tp,
        "size.0",
        BsnValue::Float(1.5),
        &registry,
        None,
    );

    assert!(
        matches!(written, Err(jackdaw_bsn::FieldWriteError::Unseeded { .. })),
        "got {written:?}"
    );
    assert_eq!(get_bsn_field(&ast, node, tp, "size"), None);
}

#[test]
fn an_unregistered_nested_struct_is_written_whole_under_its_own_type_path() {
    let mut registry = TypeRegistry::new();
    registry
        .add_registration(<Outer as bevy::reflect::GetTypeRegistration>::get_type_registration());
    let tp = Outer::type_path();
    let (mut ast, node) = one_patch_ast(empty_struct_patch(tp));

    set_bsn_field(
        &mut ast,
        node,
        tp,
        "mid.inner.v",
        BsnValue::Float(2.5),
        &registry,
    );

    let text = jackdaw_bsn::emit_entity(&ast, node);
    assert!(jackdaw_bsn::parse_bsn(&text).is_ok(), "{text}");
    match get_bsn_field(&ast, node, tp, "mid") {
        Some(BsnValue::Struct(data)) => assert_eq!(data.type_path, Mid::type_path()),
        other => panic!("expected mid as a typed struct, got {other:?}"),
    }
    assert_eq!(
        get_bsn_field(&ast, node, tp, "mid.inner.v"),
        Some(BsnValue::Float(2.5))
    );
}
