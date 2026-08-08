#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Can the editor adopt a CHANGED component shape by swapping registry
//! entries, with no restart and no ECS involvement?
//!
//! The dynamic-representation design keeps schema-reported types out of
//! the editor's ECS entirely (they live as reflection data backed by
//! the scene document), so a reload only has to update the
//! `TypeRegistry`. This probe builds the same component at two shapes
//! (v1: `{strength: f32}`, v2: `{strength: f32, label: String}`) as
//! two dylibs through the SDK pipeline, loads both into one process,
//! and answers the three questions the design hangs on:
//!
//! 1. Does a fresh registry drain see the NEWEST dylib's registration
//!    when both are loaded (inventory ordering)?
//! 2. Can a long-lived registry adopt the new shape (overwrite)?
//! 3. Does value migration work across shapes: an old dynamic value
//!    applied onto the new shape's default keeps matching fields and
//!    defaults the new ones?
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test component_shape_refresh -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use bevy::reflect::TypeRegistry;
use bevy::reflect::structs::DynamicStruct;
use jackdaw::sdk_paths::SdkPaths;

mod util;

const TYPE_PATH: &str = "shape_game::ShapeShifter";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const LIB_V1: &str = r#"//! The original shape.
use bevy::prelude::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShapeShifter {
    pub strength: f32,
}
"#;

const LIB_V2: &str = r#"//! A field added to the same type.
use bevy::prelude::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShapeShifter {
    pub strength: f32,
    pub label: String,
}
"#;

/// Build the fixture project through the SDK pipeline and copy the
/// artifact to a unique path (dlopen caches by path).
fn build_variant(sdk: &SdkPaths, source: &str, tag: &str) -> PathBuf {
    let dir = util::stage_fixture("shape_game");
    std::fs::write(dir.join("src/lib.rs"), source).expect("write variant source");
    let target = dir.join("target-fixture");
    let status = Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &sdk.triple])
        .current_dir(&dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .status()
        .expect("spawn cargo");
    assert!(status.success(), "variant {tag} failed to build");

    let built = target.join(format!(
        "{}/debug/{}shape_game{}",
        sdk.triple,
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    let load_dir = std::env::temp_dir().join(format!("jackdaw-shape-{}", std::process::id()));
    std::fs::create_dir_all(&load_dir).unwrap();
    let unique = load_dir.join(format!("shape-{tag}{}", std::env::consts::DLL_SUFFIX));
    std::fs::copy(&built, &unique).expect("copy artifact");
    unique
}

fn field_names(registry: &TypeRegistry) -> Vec<String> {
    let registration = registry
        .get_with_type_path(TYPE_PATH)
        .unwrap_or_else(|| panic!("{TYPE_PATH} not registered"));
    match registration.type_info() {
        bevy::reflect::TypeInfo::Struct(info) => {
            info.iter().map(|field| field.name().to_string()).collect()
        }
        other => panic!("unexpected type info: {other:?}"),
    }
}

#[test]
fn changed_shape_is_adoptable_without_restart() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with --features dylib --target {}",
        sdk.triple
    );

    // v1: original shape, loaded the way the editor loads a project.
    let v1 = build_variant(&sdk, LIB_V1, "v1");
    let lib1 = unsafe { libloading::Library::new(&v1) }.expect("dlopen v1");
    std::mem::forget(lib1);

    let mut registry = TypeRegistry::default();
    registry.register_derived_types();
    assert_eq!(
        field_names(&registry),
        vec!["strength"],
        "v1 shape should be {{strength}}"
    );
    println!("v1 loaded: shape = {:?}", field_names(&registry));

    // The long-lived registry an editor would hold across the reload.
    let mut app_registry = TypeRegistry::default();
    app_registry.register_derived_types();

    // Question 1: obtain exactly the NEW dylib's registrations. A
    // fresh drain cannot (register-if-absent keeps the oldest
    // submission, verified empirically), so the discriminator is a
    // pointer-set diff over the raw inventory: snapshot the submission
    // fn pointers before the dlopen, then everything new afterwards
    // belongs to the new dylib, regardless of iteration order.
    use bevy::reflect::__macro_exports::auto_register::{AutomaticReflectRegistrations, inventory};
    let before: std::collections::HashSet<usize> = inventory::iter::<AutomaticReflectRegistrations>
        .into_iter()
        .map(|s| s.0 as usize)
        .collect();

    // v2: same type path, a field added; rebuilt and loaded alongside.
    let v2 = build_variant(&sdk, LIB_V2, "v2");
    let lib2 = unsafe { libloading::Library::new(&v2) }.expect("dlopen v2");
    std::mem::forget(lib2);

    let mut new_only = TypeRegistry::default();
    let mut new_submissions = 0usize;
    for submission in inventory::iter::<AutomaticReflectRegistrations> {
        if !before.contains(&(submission.0 as usize)) {
            (submission.0)(&mut new_only);
            new_submissions += 1;
        }
    }
    println!("inventory diff found {new_submissions} submissions from the new dylib");
    assert!(
        new_submissions > 0,
        "the new dylib's submissions must be discoverable"
    );
    let new_shape = field_names(&new_only);
    println!("new dylib's registration: shape = {new_shape:?}");
    assert_eq!(
        new_shape,
        vec!["strength", "label"],
        "the pointer-set diff must yield the NEW registration"
    );

    // Question 2: can the long-lived registry adopt the new shape?
    let new_registration = new_only
        .get_with_type_path(TYPE_PATH)
        .expect("new registration present")
        .clone();
    app_registry.overwrite_registration(new_registration);
    assert_eq!(
        field_names(&app_registry),
        vec!["strength", "label"],
        "overwrite_registration must swap the shape in a live registry"
    );
    println!(
        "long-lived registry after overwrite: shape = {:?}",
        field_names(&app_registry)
    );

    // Question 3: migrate an old-shape value onto the new shape.
    // The old value is what the editor holds: pure data, no dylib code.
    let mut old_value = DynamicStruct::default();
    old_value.insert("strength", 42.5f32);

    let registration = app_registry.get_with_type_path(TYPE_PATH).unwrap();
    let reflect_default = registration
        .data::<bevy::reflect::prelude::ReflectDefault>()
        .expect("new shape keeps ReflectDefault");
    let mut migrated = reflect_default.default();
    migrated.apply(&old_value);

    let bevy::reflect::ReflectRef::Struct(migrated_ref) = migrated.reflect_ref() else {
        panic!("migrated value is not a struct");
    };
    let strength = migrated_ref
        .field("strength")
        .and_then(|f| f.try_downcast_ref::<f32>())
        .copied()
        .expect("strength survived migration");
    let label = migrated_ref
        .field("label")
        .and_then(|f| f.try_downcast_ref::<String>())
        .cloned()
        .expect("label exists on the migrated value");
    assert_eq!(strength, 42.5, "matching field must carry over");
    assert_eq!(label, String::new(), "new field must take its default");

    println!("migration: strength carried ({strength}), label defaulted ({label:?})");
    println!("changed shape adopted with no restart, no ECS, no round-trip");
}
