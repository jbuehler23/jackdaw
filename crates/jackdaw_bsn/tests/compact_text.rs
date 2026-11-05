//! The compact form scene text is written in: list elements leave out fields
//! at their defaults and nothing else does, floats read back with the same
//! bits, and older text reads the same and converts when written.

use std::any::TypeId;

use bevy::ecs::prelude::Component;
use bevy::ecs::reflect::ReflectComponent;
use bevy::math::{Quat, Vec2, Vec3};
use bevy::platform::collections::HashMap;
use bevy::prelude::{Entity, ReflectDefault};
use bevy::reflect::{FromReflect, Reflect, TypeRegistry};

use jackdaw_bsn::{
    BsnPatch, BsnPatches, BsnValue, SceneBsnAst, apply_deltas, bsn_value_eq, bsn_value_to_reflect,
    compact_document, component_to_bsn_patch, emit_entity, emit_scene, emit_value_text,
    get_bsn_field, omitted_list_field, parse_bsn_text, set_bsn_field,
};

#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
struct Row {
    offset: Vec2,
    scale: Vec2,
    weight: f32,
    label: String,
    tilt: Quat,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            scale: Vec2::ONE,
            weight: 1.0,
            label: String::new(),
            tilt: Quat::IDENTITY,
        }
    }
}

#[derive(Component, Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Holder {
    rows: Vec<Row>,
    pivot: Vec3,
    numbers: Vec<f32>,
    wide: f64,
}

fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::default();
    registry.register::<Holder>();
    registry.register::<Row>();
    registry.register::<Vec<Row>>();
    registry.register::<Vec<f32>>();
    registry.register::<Vec2>();
    registry.register::<Vec3>();
    registry.register::<Quat>();
    registry.register::<String>();
    registry.register::<f32>();
    registry.register::<f64>();
    registry
}

/// `value` as one node's only patch, and that node.
fn document_of(value: &Holder, registry: &TypeRegistry) -> (SceneBsnAst, Entity) {
    let mut ast = SceneBsnAst::default();
    let patch = ast
        .world
        .spawn(component_to_bsn_patch(value, registry))
        .id();
    let node = ast.world.spawn(BsnPatches(vec![patch])).id();
    ast.roots.push(node);
    (ast, node)
}

fn read_holder(ast: &SceneBsnAst, registry: &TypeRegistry) -> Holder {
    let node = ast.roots[0];
    let value = get_bsn_field(ast, node, Holder::type_path_for_test(), "").expect("the component");
    let reflected = bsn_value_to_reflect(&value, TypeId::of::<Holder>(), registry, None)
        .expect("the component reads");
    Holder::from_reflect(&*reflected).expect("the component converts")
}

impl Holder {
    fn type_path_for_test() -> &'static str {
        <Holder as bevy::reflect::TypePath>::type_path()
    }
}

fn holder_with_rows(rows: Vec<Row>) -> Holder {
    Holder {
        rows,
        ..Holder::default()
    }
}

#[test]
fn a_list_element_leaves_out_what_its_type_defaults_to_and_reads_back_whole() {
    let registry = registry();
    let holder = holder_with_rows(vec![
        Row::default(),
        Row {
            offset: Vec2::new(0.0, 2.0),
            label: "kept".into(),
            ..Row::default()
        },
    ]);
    let (ast, _) = document_of(&holder, &registry);
    let text = emit_scene(&ast);

    assert!(!text.contains("scale"), "{text}");
    assert!(!text.contains("weight"), "{text}");
    assert!(!text.contains("tilt"), "{text}");
    assert!(text.contains("label: \"kept\""), "{text}");
    assert!(
        text.contains("offset: glam::Vec2 { y: 2.0 }"),
        "inside a list a nested struct leaves out its own defaults too:\n{text}"
    );
    assert!(
        text.contains("    compact_text::Row,\n"),
        "an element at its default is its bare type:\n{text}"
    );

    let read = read_holder(&parse_bsn_text(&text).expect("the text parses"), &registry);
    assert_eq!(read, holder);
}

#[test]
fn a_field_that_differs_from_its_owner_default_stays_when_its_type_default_matches() {
    let registry = registry();
    // `Row::default().scale` is one, `Vec2::default()` is zero: leaving this
    // out would read back as one.
    let holder = holder_with_rows(vec![Row {
        scale: Vec2::ZERO,
        ..Row::default()
    }]);
    let (ast, _) = document_of(&holder, &registry);
    let text = emit_scene(&ast);
    assert!(text.contains("scale: glam::Vec2"), "{text}");
    let read = read_holder(&parse_bsn_text(&text).expect("the text parses"), &registry);
    assert_eq!(read, holder);
}

#[test]
fn negative_zero_is_not_a_default() {
    let registry = registry();
    let holder = holder_with_rows(vec![Row {
        offset: Vec2::new(-0.0, 0.0),
        weight: 1.0,
        ..Row::default()
    }]);
    let (ast, _) = document_of(&holder, &registry);
    let text = emit_scene(&ast);
    assert!(text.contains("x: -0.0"), "{text}");
    let read = read_holder(&parse_bsn_text(&text).expect("the text parses"), &registry);
    assert_eq!(read.rows[0].offset.x.to_bits(), (-0.0_f32).to_bits());
}

#[test]
fn a_struct_outside_a_list_keeps_its_default_fields_for_prefab_merging() {
    let registry = registry();
    let (ast, node) = document_of(
        &Holder {
            pivot: Vec3::new(1.0, 0.0, 0.0),
            ..Holder::default()
        },
        &registry,
    );
    let text = emit_scene(&ast);
    assert!(
        text.contains("pivot: glam::Vec3 { x: 1.0, y: 0.0, z: 0.0 }"),
        "{text}"
    );

    let pivot = get_bsn_field(&ast, node, Holder::type_path_for_test(), "pivot").expect("pivot");
    let mut base = BsnValue::from_reflect(&Vec3::new(5.0, 6.0, 7.0), &registry);
    apply_deltas(&mut base, &pivot);
    let merged = bsn_value_to_reflect(&base, TypeId::of::<Vec3>(), &registry, None)
        .and_then(|reflected| Vec3::from_reflect(&*reflected))
        .expect("the merged value");
    assert_eq!(
        merged,
        Vec3::new(1.0, 0.0, 0.0),
        "an override of a zero must not inherit the base's value"
    );
}

#[test]
fn a_default_valued_component_stays_a_bare_type_path() {
    let registry = registry();
    let (ast, _) = document_of(&Holder::default(), &registry);
    assert_eq!(emit_scene(&ast), "compact_text::Holder\n");
}

/// Its shortest text, passing through an `f64`, rounds to the `f32` beside it.
const READS_AS_A_NEIGHBOUR: f32 = f32::from_bits(0x15ae_43fd);

#[test]
fn floats_read_back_with_the_same_f32_bits() {
    let registry = registry();
    let numbers = vec![
        0.1,
        -0.0,
        0.0,
        1.0,
        -1.5,
        0.333_333_34,
        1.0e30,
        -1.0e30,
        f32::MAX,
        f32::MIN,
        f32::MIN_POSITIVE,
        1.0e-45,
        16_777_216.0,
        16_777_218.0,
        0.209_043_86,
        f32::INFINITY,
        f32::NEG_INFINITY,
        READS_AS_A_NEIGHBOUR,
    ];
    let holder = Holder {
        numbers: numbers.clone(),
        ..Holder::default()
    };
    let (ast, _) = document_of(&holder, &registry);
    let text = emit_scene(&ast);
    let read = read_holder(&parse_bsn_text(&text).expect("the text parses"), &registry);
    let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&read.numbers), bits(&numbers));
    assert_eq!(
        emit_scene(&parse_bsn_text(&text).expect("the text parses")),
        text,
        "the text is a fixpoint"
    );
}

#[test]
fn a_float_held_widened_as_older_text_held_it_reads_back_with_the_same_bits() {
    for value in [
        READS_AS_A_NEIGHBOUR,
        0.1_f32,
        1.0e-45,
        f32::MIN_POSITIVE,
        f32::MAX,
        0.333_333_34,
        -0.0,
    ] {
        let text = emit_value_text(&BsnValue::Float(f64::from(value)));
        let read: f64 = text.parse().expect("a number");
        assert_eq!((read as f32).to_bits(), value.to_bits(), "{text}");
    }
}

#[test]
fn an_f32_writes_its_shortest_text() {
    let registry = registry();
    let text = |value: f32| emit_value_text(&BsnValue::from_reflect(&value, &registry));
    assert_eq!(text(0.1), "0.1");
    assert_eq!(text(-0.209_043_86), "-0.20904386");
    assert_eq!(text(1.0), "1.0");
    assert_eq!(text(-0.0), "-0.0");
    assert_eq!(text(1.0e30), "1000000000000000000000000000000.0");
}

#[test]
fn a_true_f64_keeps_its_value() {
    let registry = registry();
    for wide in [0.1_f64, 1e-12, std::f64::consts::PI, 1.000_000_000_1] {
        let holder = Holder {
            wide,
            ..Holder::default()
        };
        let (ast, _) = document_of(&holder, &registry);
        let read = read_holder(
            &parse_bsn_text(&emit_scene(&ast)).expect("the text parses"),
            &registry,
        );
        assert_eq!(read.wide.to_bits(), wide.to_bits());
    }
}

const OLDER_TEXT: &str = "\
compact_text::Holder {
    rows: [
        compact_text::Row {
            offset: glam::Vec2 {
                x: 0.0,
                y: 0.0,
            },
            scale: glam::Vec2 {
                x: 1.0,
                y: 1.0,
            },
            weight: 0.10000000149011612,
            label: \"\",
            tilt: glam::Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        },
    ],
    pivot: glam::Vec3 {
        x: 0.20000000298023224,
        y: 0.0,
        z: 0.0,
    },
}
";

#[test]
fn older_text_reads_the_same_and_converts_when_written() {
    let registry = registry();
    let mut ast = parse_bsn_text(OLDER_TEXT).expect("older text parses");
    let as_read = read_holder(&ast, &registry);
    assert_eq!(as_read.rows[0].weight, 0.1);
    assert_eq!(as_read.pivot, Vec3::new(0.2, 0.0, 0.0));

    compact_document(&mut ast, &registry);
    let written = emit_scene(&ast);
    assert_eq!(
        written,
        "compact_text::Holder {\n    rows: [\n        compact_text::Row { weight: 0.1 },\n    ],\n    pivot: glam::Vec3 { x: 0.2, y: 0.0, z: 0.0 },\n}\n",
    );
    let reread = parse_bsn_text(&written).expect("the written text parses");
    assert_eq!(read_holder(&reread, &registry), as_read);
    assert_eq!(emit_scene(&reread), written, "the text is a fixpoint");
}

#[test]
fn live_values_compare_equal_to_older_text_of_the_same_values() {
    let registry = registry();
    let mut older = parse_bsn_text(OLDER_TEXT).expect("older text parses");
    let live = read_holder(&older, &registry);
    let (live_document, live_node) = document_of(&live, &registry);
    compact_document(&mut older, &registry);

    let path = Holder::type_path_for_test();
    let older_node = older.roots[0];
    let baseline = get_bsn_field(&older, older_node, path, "").expect("the baseline");
    let current = get_bsn_field(&live_document, live_node, path, "").expect("the live value");
    assert!(
        bsn_value_eq(&baseline, &current),
        "{}\nagainst\n{}",
        emit_value_text(&baseline),
        emit_value_text(&current),
    );
    assert!(
        jackdaw_bsn::shallow_diff(&baseline, &current).is_none(),
        "nothing about these values is an override"
    );
}

#[test]
fn setting_a_field_a_list_element_left_out_writes_it() {
    let registry = registry();
    let (mut ast, node) = document_of(&holder_with_rows(vec![Row::default()]), &registry);
    let path = Holder::type_path_for_test();
    set_bsn_field(
        &mut ast,
        node,
        path,
        "rows.0.offset.x",
        BsnValue::Float(3.0),
        &registry,
    );
    assert_eq!(
        read_holder(&ast, &registry).rows[0].offset,
        Vec2::new(3.0, 0.0)
    );

    set_bsn_field(
        &mut ast,
        node,
        path,
        "rows.0.offset.x",
        BsnValue::Float(0.0),
        &registry,
    );
    assert!(
        emit_scene(&ast).contains("rows: [compact_text::Row]"),
        "back at its default, the element is written as one:\n{}",
        emit_scene(&ast)
    );
}

const BRUSH: &str = "jackdaw_scene_types::types::Brush";

fn brush_text(position_y: f32) -> String {
    format!(
        "#Block
{BRUSH} {{
    faces: [
        a::Face {{ plane: a::Plane {{ normal: glam::Vec3 {{ y: 1.0 }}, distance: 1.5 }}, material: \"m.bsn\" }},
    ],
    topology: a::Topology {{
        verts: [
            a::Vert {{ position: glam::Vec3 {{ x: 1.0, y: {position_y:?} }} }},
            a::Vert {{ position: glam::Vec3 {{ z: 1.0 }} }},
        ],
    }},
}}
"
    )
}

#[test]
fn a_brush_keeps_one_row_per_line_with_no_indent_and_a_moved_vertex_is_one_line() {
    let text = emit_scene(&parse_bsn_text(&brush_text(2.0)).expect("the brush parses"));
    assert_eq!(
        text,
        format!(
            "#Block
{BRUSH} {{
faces: [
a::Face {{ plane: a::Plane {{ normal: glam::Vec3 {{ y: 1.0 }}, distance: 1.5 }}, material: \"m.bsn\" }},
],
topology: a::Topology {{
verts: [
a::Vert {{ position: glam::Vec3 {{ x: 1.0, y: 2.0 }} }},
a::Vert {{ position: glam::Vec3 {{ z: 1.0 }} }},
],
}},
}}
"
        )
    );
    assert_eq!(
        emit_scene(&parse_bsn_text(&text).expect("the output parses")),
        text
    );

    let moved = emit_scene(&parse_bsn_text(&brush_text(3.5)).expect("the brush parses"));
    let changed = text
        .lines()
        .zip(moved.lines())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 1);
}

#[test]
fn a_child_inside_children_keeps_its_indent_outside_the_brush() {
    let source = format!(
        "#Root\nbevy_ecs::hierarchy::Children [\n    #Kid\n    {BRUSH} {{ faces: [a::F {{ x: 1.0 }}] }}\n]\n"
    );
    let ast = parse_bsn_text(&source).expect("the source parses");
    let text = emit_entity(&ast, ast.roots[0]);
    assert_eq!(
        text,
        format!(
            "#Root\nbevy_ecs::hierarchy::Children [\n    #Kid\n    {BRUSH} {{\nfaces: [\na::F {{ x: 1.0 }},\n],\n    }}\n]\n"
        )
    );
}

#[test]
fn a_value_too_large_for_one_line_keeps_a_field_per_line() {
    let fields: Vec<String> = (0..17).map(|i| format!("f{i}: {i}")).collect();
    let source = format!("a::Owner {{ big: a::Big {{ {} }} }}\n", fields.join(", "));
    let text = emit_scene(&parse_bsn_text(&source).expect("the source parses"));
    assert!(text.contains("big: a::Big {\n        f0: 0,\n"), "{text}");
    assert_eq!(
        emit_scene(&parse_bsn_text(&text).expect("the output parses")),
        text
    );

    let fields: Vec<String> = (0..16).map(|i| format!("f{i}: {i}")).collect();
    let source = format!("a::Owner {{ big: a::Big {{ {} }} }}\n", fields.join(", "));
    let text = emit_scene(&parse_bsn_text(&source).expect("the source parses"));
    assert!(text.contains("big: a::Big { f0: 0, f1: 1,"), "{text}");
}

#[test]
fn a_patch_never_collapses_its_own_fields_onto_one_line() {
    let text = emit_scene(&parse_bsn_text("a::Owner { x: 1.0, y: 2.0 }\n").expect("parses"));
    assert_eq!(text, "a::Owner {\n    x: 1.0,\n    y: 2.0,\n}\n");
    let parsed = parse_bsn_text(&text).expect("parses");
    assert!(matches!(
        parsed.get_patch(parsed.get_patches(parsed.roots[0]).expect("patches").0[0]),
        Some(BsnPatch::Struct(_))
    ));
}

#[derive(Reflect, Clone, Debug, PartialEq)]
enum Shape {
    Box { size: Vec3 },
}

impl Default for Shape {
    fn default() -> Self {
        Self::Box { size: Vec3::ONE }
    }
}

#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
struct Bag {
    held: HashMap<String, Vec3>,
    rows: HashMap<String, Row>,
}

/// An element whose default holds a map, so a map can match it or not.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default)]
struct Zeroed {
    weights: HashMap<String, f32>,
}

impl Default for Zeroed {
    fn default() -> Self {
        Self {
            weights: HashMap::from_iter([("a".to_string(), 0.0)]),
        }
    }
}

#[derive(Component, Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Nest {
    shapes: Vec<Shape>,
    maybes: Vec<Option<Vec3>>,
    bags: Vec<Bag>,
    zeroed: Vec<Zeroed>,
}

fn nest_registry() -> TypeRegistry {
    let mut registry = registry();
    registry.register::<Nest>();
    registry.register::<Shape>();
    registry.register::<Bag>();
    registry.register::<Zeroed>();
    registry.register::<Vec<Shape>>();
    registry.register::<Vec<Option<Vec3>>>();
    registry.register::<Option<Vec3>>();
    registry.register::<Vec<Bag>>();
    registry.register::<Vec<Zeroed>>();
    registry.register::<HashMap<String, Vec3>>();
    registry.register::<HashMap<String, Row>>();
    registry.register::<HashMap<String, f32>>();
    registry
}

fn nest_document(value: &Nest, registry: &TypeRegistry) -> (SceneBsnAst, Entity) {
    let mut ast = SceneBsnAst::default();
    let patch = ast
        .world
        .spawn(component_to_bsn_patch(value, registry))
        .id();
    let node = ast.world.spawn(BsnPatches(vec![patch])).id();
    ast.roots.push(node);
    (ast, node)
}

fn read_nest(ast: &SceneBsnAst, registry: &TypeRegistry) -> Nest {
    let path = <Nest as bevy::reflect::TypePath>::type_path();
    let value = get_bsn_field(ast, ast.roots[0], path, "").expect("the component");
    let reflected = bsn_value_to_reflect(&value, TypeId::of::<Nest>(), registry, None)
        .expect("the component reads");
    Nest::from_reflect(&*reflected).expect("the component converts")
}

#[test]
fn a_write_into_an_omitted_nested_field_keeps_its_owner_default_for_the_rest() {
    let registry = registry();
    let (mut ast, node) = document_of(&holder_with_rows(vec![Row::default()]), &registry);
    let path = Holder::type_path_for_test();
    let set = |ast: &mut SceneBsnAst, field: &str, value: f64| {
        set_bsn_field(ast, node, path, field, BsnValue::Float(value), &registry);
    };

    set(&mut ast, "rows[0].scale.x", 2.0);
    assert_eq!(
        read_holder(&ast, &registry).rows[0].scale,
        Vec2::new(2.0, 1.0)
    );
    set(&mut ast, "rows.0.tilt.x", 0.5);
    assert_eq!(
        read_holder(&ast, &registry).rows[0].tilt,
        Quat::from_xyzw(0.5, 0.0, 0.0, 1.0)
    );

    let reread = parse_bsn_text(&emit_scene(&ast)).expect("the text parses");
    assert_eq!(
        read_holder(&reread, &registry),
        read_holder(&ast, &registry)
    );

    set(&mut ast, "rows[0].scale.x", 1.0);
    set(&mut ast, "rows.0.tilt.x", 0.0);
    assert!(
        emit_scene(&ast).contains("rows: [compact_text::Row]"),
        "{}",
        emit_scene(&ast)
    );
}

#[test]
fn a_write_outside_a_list_stays_sparse() {
    let registry = registry();
    let (mut ast, node) = document_of(&Holder::default(), &registry);
    let path = Holder::type_path_for_test();
    set_bsn_field(
        &mut ast,
        node,
        path,
        "pivot.x",
        BsnValue::Float(2.0),
        &registry,
    );
    assert!(
        emit_scene(&ast).contains("pivot: glam::Vec3 { x: 2.0 }"),
        "{}",
        emit_scene(&ast)
    );
}

#[test]
fn what_an_omitted_list_field_holds_is_its_owner_default() {
    let registry = registry();
    let (mut ast, node) = document_of(&holder_with_rows(vec![Row::default()]), &registry);
    let path = Holder::type_path_for_test();
    let held =
        |ast: &SceneBsnAst, field: &str| omitted_list_field(ast, node, path, field, &registry);

    assert_eq!(held(&ast, "rows[0].weight"), Some(BsnValue::Float(1.0)));
    assert_eq!(held(&ast, "rows.0.scale.y"), Some(BsnValue::Float(1.0)));
    assert_eq!(held(&ast, "rows[0].tilt.w"), Some(BsnValue::Float(1.0)));
    assert_eq!(held(&ast, "pivot.x"), None, "outside a list it is sparse");
    assert_eq!(held(&ast, "rows[1].weight"), None, "no such element");

    set_bsn_field(
        &mut ast,
        node,
        path,
        "rows[0].scale.x",
        BsnValue::Float(2.0),
        &registry,
    );
    assert_eq!(held(&ast, "rows[0].scale.y"), None, "it is written now");
}

#[test]
fn a_write_reaches_through_a_bare_value_in_an_enum_an_option_and_a_map() {
    let registry = nest_registry();
    let nest = Nest {
        shapes: vec![Shape::Box { size: Vec3::ZERO }],
        maybes: vec![Some(Vec3::ZERO)],
        bags: vec![Bag {
            held: HashMap::from_iter([("a".to_string(), Vec3::ZERO)]),
            rows: HashMap::from_iter([("r".to_string(), Row::default())]),
        }],
        zeroed: Vec::new(),
    };
    let (mut ast, node) = nest_document(&nest, &registry);
    let text = emit_scene(&ast);
    assert!(text.contains("size: glam::Vec3"), "{text}");
    let path = <Nest as bevy::reflect::TypePath>::type_path();
    for field in ["shapes[0].size.x", "maybes[0].0.x", "bags[0].held[a].x"] {
        assert_eq!(
            omitted_list_field(&ast, node, path, field, &registry),
            Some(BsnValue::Float(0.0)),
            "{field}"
        );
        set_bsn_field(&mut ast, node, path, field, BsnValue::Float(1.0), &registry);
    }
    set_bsn_field(
        &mut ast,
        node,
        path,
        "bags[0].rows[r].scale.x",
        BsnValue::Float(2.0),
        &registry,
    );

    let read = read_nest(&ast, &registry);
    let one = Vec3::new(1.0, 0.0, 0.0);
    assert_eq!(read.shapes[0], Shape::Box { size: one });
    assert_eq!(read.maybes[0], Some(one));
    assert_eq!(read.bags[0].held["a"], one);
    assert_eq!(read.bags[0].rows["r"].scale, Vec2::new(2.0, 1.0));
}

#[test]
fn negative_zero_in_a_map_is_not_the_default_map() {
    let registry = nest_registry();
    let nest = Nest {
        zeroed: vec![
            Zeroed::default(),
            Zeroed {
                weights: HashMap::from_iter([("a".to_string(), -0.0)]),
            },
        ],
        ..Nest::default()
    };
    let (ast, _) = nest_document(&nest, &registry);
    let reread = parse_bsn_text(&emit_scene(&ast)).expect("the text parses");
    let read = read_nest(&reread, &registry);
    assert_eq!(read.zeroed[0].weights["a"].to_bits(), 0.0_f32.to_bits());
    assert_eq!(read.zeroed[1].weights["a"].to_bits(), (-0.0_f32).to_bits());
}

#[test]
fn the_emitter_writes_a_float_exactly_as_held() {
    for (value, text) in [
        (4_294_967_296.0, "4294967296.0"),
        (9_007_199_254_740_992.0, "9007199254740992.0"),
        (0.100_000_001_490_116_12, "0.10000000149011612"),
        (0.1, "0.1"),
        (-0.0, "-0.0"),
        (1.0, "1.0"),
    ] {
        assert_eq!(emit_value_text(&BsnValue::Float(value)), text);
    }
}

#[test]
fn a_document_float_compares_exactly() {
    let widened = f64::from(0.1_f32);
    assert!(!bsn_value_eq(
        &BsnValue::Float(0.1),
        &BsnValue::Float(widened)
    ));
    assert!(bsn_value_eq(
        &BsnValue::Float(widened),
        &BsnValue::Float(widened)
    ));
}

#[test]
fn compacting_shortens_only_fields_known_to_be_f32() {
    let registry = registry();
    let long = "0.10000000149011612";
    let text = format!(
        "compact_text::Holder {{ numbers: [{long}], wide: {long}, pivot: glam::Vec3 {{ x: {long} }} }}\n\
         #Other\nmy_game::Unknown {{ reach: {long}, rows: [my_game::Row {{ at: {long} }}] }}\n"
    );
    let mut ast = parse_bsn_text(&text).expect("the text parses");
    assert_eq!(
        emit_scene(&ast).matches(long).count(),
        5,
        "read and written alone, no value changes"
    );
    compact_document(&mut ast, &registry);
    let written = emit_scene(&ast);
    assert!(written.contains("numbers: [0.1]"), "{written}");
    assert!(written.contains("x: 0.1"), "{written}");
    assert!(written.contains(&format!("wide: {long}")), "{written}");
    assert!(written.contains(&format!("reach: {long}")), "{written}");
    assert!(written.contains(&format!("at: {long}")), "{written}");
}

#[test]
fn compacting_shortens_an_f32_inside_an_enum_an_option_and_a_map() {
    let registry = nest_registry();
    let long = "0.10000000149011612";
    let text = format!(
        "compact_text::Nest {{ shapes: [compact_text::Shape::Box {{ size: glam::Vec3 {{ x: {long} }} }}], \
         maybes: [core::option::Option::Some(glam::Vec3 {{ x: {long} }})], \
         bags: [compact_text::Bag {{ held: map[(\"a\", glam::Vec3 {{ x: {long} }})] }}] }}\n"
    );
    let mut ast = parse_bsn_text(&text).expect("the text parses");
    let before = read_nest(&ast, &registry);
    compact_document(&mut ast, &registry);
    let written = emit_scene(&ast);
    assert!(!written.contains(long), "{written}");
    let reread = parse_bsn_text(&written).expect("the written text parses");
    assert_eq!(read_nest(&reread, &registry), before);
}

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
enum Speed {
    Fixed(f32),
    Ramp { to: f32 },
}

#[test]
fn a_component_spelled_by_its_variant_holds_its_floats_as_shortest_text() {
    let mut registry = TypeRegistry::default();
    registry.register::<Speed>();
    registry.register::<f32>();
    let path = <Speed as bevy::reflect::TypePath>::type_path();
    for (text, want) in [
        (
            format!("{path}::Fixed(0.10000000149011612)\n"),
            format!("{path}::Fixed(0.1)"),
        ),
        (
            format!("{path}::Ramp {{ to: 0.10000000149011612 }}\n"),
            "to: 0.1,".to_string(),
        ),
    ] {
        let mut ast = parse_bsn_text(&text).expect("it parses");
        compact_document(&mut ast, &registry);
        let written = emit_scene(&ast);
        assert!(written.contains(&want), "{written}");
    }
    let _ = (Speed::Fixed(0.0), Speed::Ramp { to: 0.0 });
}

#[derive(Reflect, Clone, Debug)]
#[reflect(Default, no_field_bounds)]
struct Tree {
    label: String,
    kids: Vec<Tree>,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            label: String::new(),
            kids: vec![Self {
                label: String::new(),
                kids: Vec::new(),
            }],
        }
    }
}

#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
struct Forest {
    trees: Vec<Tree>,
}

#[test]
fn a_default_holding_its_own_type_compacts_and_reads_back() {
    let mut registry = TypeRegistry::default();
    registry.register::<Forest>();
    registry.register::<Tree>();
    registry.register::<Vec<Tree>>();
    registry.register::<String>();
    let forest = Forest {
        trees: vec![
            Tree::default(),
            Tree {
                label: "oak".to_string(),
                kids: Vec::new(),
            },
        ],
    };
    let mut ast = SceneBsnAst::default();
    let patch = ast
        .world
        .spawn(component_to_bsn_patch(&forest, &registry))
        .id();
    let node = ast.world.spawn(BsnPatches(vec![patch])).id();
    ast.roots.push(node);
    let path = <Forest as bevy::reflect::TypePath>::type_path();
    let value = get_bsn_field(&ast, node, path, "").expect("the component");
    let reflected =
        bsn_value_to_reflect(&value, TypeId::of::<Forest>(), &registry, None).expect("it reads");
    let read = Forest::from_reflect(&*reflected).expect("it converts");
    assert_eq!(read.trees.len(), 2);
    assert_eq!(read.trees[0].kids.len(), 1);
    assert_eq!(read.trees[1].label, "oak");
    assert!(read.trees[1].kids.is_empty());
}
