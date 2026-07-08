//! Write-path tests: document AST -> `.bsn` text, and the parse/emit round
//! trip.

use bevy::ecs::entity::Entity;

use jackdaw_bsn::{
    BsnField, BsnPatch, BsnPatches, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    SceneBsnAst, emit_scene, parse_bsn_text,
};

/// Recursively assert that two document ASTs are structurally identical:
/// same roots (in order), same patches per node (in order, same variant and
/// payload), same nested field/value structure.
fn assert_ast_eq(a: &SceneBsnAst, b: &SceneBsnAst) {
    assert_eq!(a.roots.len(), b.roots.len(), "root count differs");
    for (&ra, &rb) in a.roots.iter().zip(b.roots.iter()) {
        assert_patches_eq(a, ra, b, rb);
    }
}

fn assert_patches_eq(a: &SceneBsnAst, na: Entity, b: &SceneBsnAst, nb: Entity) {
    let pa = a.get_patches(na).expect("node in `a` has patches");
    let pb = b.get_patches(nb).expect("node in `b` has patches");
    assert_eq!(pa.0.len(), pb.0.len(), "patch count differs for node");

    for (&ea, &eb) in pa.0.iter().zip(pb.0.iter()) {
        let patch_a = a.get_patch(ea).expect("patch component in `a`");
        let patch_b = b.get_patch(eb).expect("patch component in `b`");
        assert_patch_eq(a, patch_a, b, patch_b);
    }
}

fn assert_patch_eq(a: &SceneBsnAst, patch_a: &BsnPatch, b: &SceneBsnAst, patch_b: &BsnPatch) {
    match (patch_a, patch_b) {
        (BsnPatch::Name(na), BsnPatch::Name(nb)) => assert_eq!(na, nb, "Name patch"),
        (BsnPatch::Base(pa), BsnPatch::Base(pb)) => assert_eq!(pa, pb, "Base patch"),
        (BsnPatch::Type(ta), BsnPatch::Type(tb)) => assert_eq!(ta, tb, "Type patch"),
        (BsnPatch::Struct(da), BsnPatch::Struct(db)) => {
            assert_eq!(da.type_path, db.type_path, "Struct type_path");
            assert_fields_eq(&da.fields, &db.fields);
        }
        (BsnPatch::TupleStruct(da), BsnPatch::TupleStruct(db)) => {
            assert_eq!(da.type_path, db.type_path, "TupleStruct type_path");
            assert_values_eq(&da.values, &db.values);
        }
        (BsnPatch::Template(ta, fa), BsnPatch::Template(tb, fb)) => {
            assert_eq!(ta, tb, "Template type_path");
            match (fa, fb) {
                (Some(fa), Some(fb)) => assert_fields_eq(fa, fb),
                (None, None) => {}
                _ => panic!("Template field presence mismatch"),
            }
        }
        (BsnPatch::Children(ca), BsnPatch::Children(cb)) => {
            assert_eq!(ca.len(), cb.len(), "Children count");
            for (&child_a, &child_b) in ca.iter().zip(cb.iter()) {
                assert_patches_eq(a, child_a, b, child_b);
            }
        }
        _ => panic!(
            "patch variant mismatch: {} vs {}",
            describe_patch(patch_a),
            describe_patch(patch_b)
        ),
    }
}

/// Small test-only description of a patch's variant, for panic messages.
/// [`BsnPatch`] doesn't derive `Debug` in the document model (not needed by
/// editor code), so this crate can't implement it here (orphan rule).
fn describe_patch(patch: &BsnPatch) -> String {
    match patch {
        BsnPatch::Name(n) => format!("Name({n})"),
        BsnPatch::Base(p) => format!("Base({p})"),
        BsnPatch::Type(t) => format!("Type({t})"),
        BsnPatch::Struct(d) => format!("Struct({})", d.type_path),
        BsnPatch::TupleStruct(d) => format!("TupleStruct({})", d.type_path),
        BsnPatch::Template(t, _) => format!("Template({t})"),
        BsnPatch::Children(c) => format!("Children(len={})", c.len()),
    }
}

fn assert_fields_eq(fa: &BsnStructFields, fb: &BsnStructFields) {
    assert_eq!(fa.0.len(), fb.0.len(), "field count");
    for (a, b) in fa.0.iter().zip(fb.0.iter()) {
        assert_eq!(a.name, b.name, "field name");
        assert_value_eq(&a.value, &b.value);
    }
}

fn assert_values_eq(va: &[BsnValue], vb: &[BsnValue]) {
    assert_eq!(va.len(), vb.len(), "value count");
    for (a, b) in va.iter().zip(vb.iter()) {
        assert_value_eq(a, b);
    }
}

fn assert_value_eq(a: &BsnValue, b: &BsnValue) {
    match (a, b) {
        (BsnValue::Float(x), BsnValue::Float(y)) => {
            assert!((x - y).abs() < 1e-6, "float value {x} vs {y}");
        }
        (BsnValue::Int(x), BsnValue::Int(y)) => assert_eq!(x, y, "int value"),
        (BsnValue::Bool(x), BsnValue::Bool(y)) => assert_eq!(x, y, "bool value"),
        (BsnValue::String(x), BsnValue::String(y)) => assert_eq!(x, y, "string value"),
        (BsnValue::Type(x), BsnValue::Type(y)) => assert_eq!(x, y, "type value"),
        (BsnValue::Struct(x), BsnValue::Struct(y)) => {
            assert_eq!(x.type_path, y.type_path, "nested struct type_path");
            assert_fields_eq(&x.fields, &y.fields);
        }
        (BsnValue::TupleStruct(x), BsnValue::TupleStruct(y)) => {
            assert_eq!(x.type_path, y.type_path, "nested tuple struct type_path");
            assert_values_eq(&x.values, &y.values);
        }
        (BsnValue::List(x), BsnValue::List(y)) => assert_values_eq(x, y),
        _ => panic!(
            "value variant mismatch: {} vs {}",
            describe_value(a),
            describe_value(b)
        ),
    }
}

/// Small test-only description of a value's variant, for panic messages.
fn describe_value(value: &BsnValue) -> String {
    match value {
        BsnValue::Float(v) => format!("Float({v})"),
        BsnValue::Int(v) => format!("Int({v})"),
        BsnValue::Bool(v) => format!("Bool({v})"),
        BsnValue::String(v) => format!("String({v})"),
        BsnValue::Type(v) => format!("Type({v})"),
        BsnValue::Struct(d) => format!("Struct({})", d.type_path),
        BsnValue::TupleStruct(d) => format!("TupleStruct({})", d.type_path),
        BsnValue::List(v) => format!("List(len={})", v.len()),
    }
}

#[test]
fn roundtrip_preserves_structure_and_emitted_text_is_stable() {
    let original_text = include_str!("fixtures/example.bsn");

    let ast1 = parse_bsn_text(original_text).expect("fixture should parse");
    let emitted1 = emit_scene(&ast1);

    let ast2 = parse_bsn_text(&emitted1).expect("re-emitted text should re-parse");
    let emitted2 = emit_scene(&ast2);

    // Structural equality: same roots, same patches per node, same values.
    assert_ast_eq(&ast1, &ast2);

    // Text-level fixpoint: emitting the re-parsed document reproduces the
    // same text (no drift across a second round trip).
    assert_eq!(emitted1, emitted2, "emitted text must be stable under re-emission");

    // One more round trip for good measure, matching the exact assertion
    // shape requested: emit(parse(emit(parse(x)))) == emit(parse(x)).
    let ast3 = parse_bsn_text(&emitted2).expect("second re-emitted text should re-parse");
    let emitted3 = emit_scene(&ast3);
    assert_eq!(emitted2, emitted3);
}

#[test]
fn emit_is_byte_identical_across_repeated_calls() {
    let text = include_str!("fixtures/example.bsn");
    let ast = parse_bsn_text(text).expect("fixture should parse");

    let a = emit_scene(&ast);
    let b = emit_scene(&ast);
    assert_eq!(a, b, "emitting the same document twice must be byte-identical");
}

#[test]
fn emit_preserves_document_field_order() {
    // Construct a document by hand with fields in a specific, non-alphabetical
    // order, and assert the emitted text matches that order exactly.
    let mut ast = SceneBsnAst::default();

    let patch = ast
        .world
        .spawn(BsnPatch::Struct(BsnStructData {
            type_path: "test::Ordered".into(),
            fields: BsnStructFields(vec![
                BsnField {
                    name: "z_field".into(),
                    value: BsnValue::Int(1),
                },
                BsnField {
                    name: "a_field".into(),
                    value: BsnValue::Int(2),
                },
                BsnField {
                    name: "m_field".into(),
                    value: BsnValue::Int(3),
                },
            ]),
        }))
        .id();
    let entity = ast.world.spawn(BsnPatches(vec![patch])).id();
    ast.roots.push(entity);

    let text = emit_scene(&ast);
    let expected = "test::Ordered {\n    z_field: 1,\n    a_field: 2,\n    m_field: 3,\n}\n";
    assert_eq!(text, expected, "field emission must follow document Vec order");
}

#[test]
fn emit_tuple_struct_string_value_roundtrips() {
    // Covers the non-asset path for a Handle-shaped field: the fixture's
    // SceneRoot("...") tuple struct carries its asset path as a plain string
    // (component_to_bsn_patch, not component_to_bsn_patch_with_assets, since
    // there is no live AssetServer in this test). This documents the gap
    // noted in the task report: the _with_assets variant needs an AssetServer
    // and ReflectHandle-registered type to exercise the Handle-resolution
    // branch, which is impractical without a running App.
    let mut ast = SceneBsnAst::default();

    let patch = ast
        .world
        .spawn(BsnPatch::TupleStruct(BsnTupleStructData {
            type_path: "bevy_scene::components::SceneRoot".into(),
            values: vec![BsnValue::String("models/Foo.gltf#Scene0".into())],
        }))
        .id();
    let entity = ast.world.spawn(BsnPatches(vec![patch])).id();
    ast.roots.push(entity);

    let text = emit_scene(&ast);
    assert!(text.contains("SceneRoot(\"models/Foo.gltf#Scene0\")"));

    let reparsed = parse_bsn_text(&text).expect("should re-parse");
    assert_eq!(reparsed.roots.len(), 1);
}

#[test]
fn component_to_bsn_patch_with_assets_resolves_handle_to_path() {
    use bevy::app::{App, TaskPoolPlugin};
    use bevy::asset::{Asset, AssetApp, AssetPlugin, AssetServer, Handle, ReflectHandle};
    use bevy::ecs::reflect::AppTypeRegistry;
    use bevy::reflect::Reflect;
    use jackdaw_bsn::{BsnAssetContext, component_to_bsn_patch_with_assets};
    use std::path::Path;

    #[derive(Asset, Reflect, Default)]
    struct ProbeAsset;

    #[derive(Reflect)]
    struct HasHandle {
        target: Handle<ProbeAsset>,
    }

    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()));
    app.init_asset::<ProbeAsset>();

    let registry = AppTypeRegistry::default();
    registry.write().register::<HasHandle>();
    registry.write().register::<Handle<ProbeAsset>>();
    registry
        .write()
        .register_type_data::<Handle<ProbeAsset>, ReflectHandle>();
    app.world_mut().insert_resource(registry);

    let asset_server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<ProbeAsset> = asset_server.load("probes/thing.probe");

    let component = HasHandle { target: handle };
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let reg = registry.read();

    let ctx = BsnAssetContext {
        asset_server: &asset_server,
        parent_path: Path::new(""),
    };
    let patch = component_to_bsn_patch_with_assets(&component, &reg, &ctx);

    let BsnPatch::Struct(data) = patch else {
        panic!("expected a Struct patch");
    };
    let field = data
        .fields
        .0
        .iter()
        .find(|f| f.name == "target")
        .expect("target field emitted");
    match &field.value {
        BsnValue::String(path) => assert_eq!(path, "probes/thing.probe"),
        other => panic!("expected the handle to resolve to a path string, got {}", describe_value(other)),
    }
}
