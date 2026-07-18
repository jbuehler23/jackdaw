//! Spike 1 probe: `reflect_auto_register` across `dlopen` with the shared SDK.
//!
//! Builds `.scratch/project-onboarding/spike1/spike_game` (a plain Bevy
//! cdylib deriving `Reflect` on one component, with NO registration code,
//! NO export macro, and NO build script) through the SDK pipeline, dlopens
//! it, and checks whether the component appears when a fresh registry
//! drains the shared inventory list.
//!
//! Requires the `dylib` feature and an explicit `--target` (the host
//! triple): the test binary must link the same triple-dir
//! `libjackdaw_sdk` the spike dylib links.
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test spike_auto_register -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use bevy::reflect::TypeRegistry;
use jackdaw::sdk_paths::SdkPaths;

const SPIKE_TYPE_PATH: &str = "spike_game::SpikeAutoComponent";

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

    let spike_dir = workspace_root().join(".scratch/project-onboarding/spike1/spike_game");
    let spike_target = spike_dir.join("target-spike");
    // Wrapper behavior is not part of cargo's fingerprint; build from
    // clean so stale units cannot poison the probe.
    let _ = std::fs::remove_dir_all(&spike_target);

    // Build the spike project through the SDK pipeline, isolated target
    // dir. The crate type is a build flag, never a manifest entry: the
    // Rust dylib keeps the .rustc metadata section (linkage identity,
    // spike 3) that a cdylib would strip.
    let status = Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &triple])
        .current_dir(&spike_dir)
        .env("CARGO_TARGET_DIR", &spike_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_WRAPPER_LOG", "1")
        .status()
        .expect("spawn cargo for the spike project");
    assert!(status.success(), "spike project failed to build");

    let spike_dylib = spike_target.join(format!(
        "{triple}/debug/{}spike_game{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(
        spike_dylib.exists(),
        "spike dylib missing at {}",
        spike_dylib.display()
    );

    // Negative control: before the dylib loads, a fresh registry drain must
    // not know the spike type (proves the positive result comes from the
    // dlopen, not from the test binary's own inventory).
    let mut before = TypeRegistry::default();
    before.register_derived_types();
    assert!(
        before.get_with_type_path(SPIKE_TYPE_PATH).is_none(),
        "spike type visible before dlopen; the probe is not isolating"
    );

    // dlopen runs the spike dylib's constructors, which push inventory
    // submissions into the shared bevy_reflect inside libjackdaw_sdk.
    let lib = unsafe { libloading::Library::new(&spike_dylib) }.expect("dlopen the spike dylib");
    // Never unloaded; leak deliberately, mirroring the loader's rule.
    std::mem::forget(lib);

    let mut after = TypeRegistry::default();
    after.register_derived_types();
    let registration = after.get_with_type_path(SPIKE_TYPE_PATH);
    assert!(
        registration.is_some(),
        "SPIKE FAILED: {SPIKE_TYPE_PATH} not in the registry after dlopen + \
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

    println!("SPIKE PASSED: {SPIKE_TYPE_PATH} auto-registered across dlopen");
}
