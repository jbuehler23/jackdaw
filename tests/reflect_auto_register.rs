#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! `reflect_auto_register` across `dlopen` with the shared SDK.
//!
//! Builds `tests/fixtures/reflect_game` (a plain Bevy
//! cdylib deriving `Reflect` on one component, with NO registration code,
//! NO export macro, and NO build script) through the SDK pipeline, dlopens
//! it, and checks whether the component appears when a fresh registry
//! drains the shared inventory list.
//!
//! Requires the `dylib` feature and an explicit `--target` (the host
//! triple): the test binary must link the same triple-dir
//! `libjackdaw_sdk` the fixture dylib links.
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test reflect_auto_register -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;

use bevy::reflect::TypeRegistry;
use jackdaw::sdk_paths::SdkPaths;

mod util;

const COMPONENT_TYPE_PATH: &str = "reflect_game::AutoRegisteredComponent";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn auto_registered_types_cross_the_dlopen_boundary() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing at {}; build with `cargo build -p jackdaw --features dylib --target {triple}`",
        sdk.dylib.display()
    );
    assert!(
        sdk.wrapper_exists(),
        "rustc wrapper missing at {}",
        sdk.wrapper.display()
    );

    // Build through the real generated-shim path. This pins the fixture's
    // closure to the SDK lock and supplies the same per-edge extern plan an
    // editor-created game or marketplace extension receives.
    let fixture_dylib = util::build_reflect_fixture(&sdk).dylib;
    assert!(
        fixture_dylib.exists(),
        "fixture dylib missing at {}",
        fixture_dylib.display()
    );

    // Negative control: before the dylib loads, a fresh registry drain must
    // not know the fixture type (proves the positive result comes from the
    // dlopen, not from the test binary's own inventory).
    let mut before = TypeRegistry::default();
    before.register_derived_types();
    assert!(
        before.get_with_type_path(COMPONENT_TYPE_PATH).is_none(),
        "fixture type visible before dlopen; the probe is not isolating"
    );

    // dlopen runs the fixture dylib's constructors, which push inventory
    // submissions into the shared bevy_reflect inside libjackdaw_sdk.
    let lib =
        unsafe { libloading::Library::new(&fixture_dylib) }.expect("dlopen the fixture dylib");
    // Never unloaded; leak deliberately, mirroring the loader's rule.
    std::mem::forget(lib);

    let mut after = TypeRegistry::default();
    after.register_derived_types();
    let registration = after.get_with_type_path(COMPONENT_TYPE_PATH);
    assert!(
        registration.is_some(),
        "{COMPONENT_TYPE_PATH} not in the registry after dlopen + \
         register_derived_types; auto-register did not cross the boundary"
    );

    // The registration must be usable, not just present: reflect data intact.
    let registration = registration.unwrap();
    assert!(
        registration
            .data::<bevy::ecs::reflect::ReflectComponent>()
            .is_some(),
        "registration lacks ReflectComponent data"
    );
    assert!(
        registration
            .data::<bevy::reflect::prelude::ReflectDefault>()
            .is_some(),
        "registration lacks ReflectDefault data (component picker filters on it)"
    );

    println!("{COMPONENT_TYPE_PATH} auto-registered across dlopen");
}
