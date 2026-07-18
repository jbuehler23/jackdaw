//! Spike 2 probe, now the pipeline's regression test: whole-graph
//! extern redirect with an ecosystem dependency.
//!
//! Builds `.scratch/project-onboarding/spike2/spike_game2` (bevy +
//! avian3d, one Reflect-derived component, zero registration code)
//! through the production plan pipeline
//! ([`jackdaw::project_build::plan`]), dlopens the result, and checks
//! that both the user's component and avian's own reflected types
//! cross the boundary.
//!
//! The test must run with an explicit `--target` (the host triple) so
//! the test binary links the same triple-dir SDK dylib the spike dylib
//! is built against:
//!
//! ```text
//! cargo test --features dylib --target <host-triple> \
//!     --test spike_ecosystem_redirect -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use bevy::reflect::TypeRegistry;
use jackdaw::project_build::plan::{SdkManifest, write_plan};
use jackdaw::sdk_paths::SdkPaths;

const SPIKE_TYPE_PATH: &str = "spike_game2::SpikePhysicsBody";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn ecosystem_dependency_compiles_and_registers_across_the_boundary() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing at {}; build with `cargo build -p jackdaw --features dylib --target {triple}`",
        sdk.dylib.display()
    );
    assert!(sdk.wrapper_exists(), "wrapper missing");

    let spike_dir = workspace_root().join(".scratch/project-onboarding/spike2/spike_game2");
    let spike_target = spike_dir.join("target-spike");
    let map_path = spike_dir.join("extern_map.txt");

    // The production pipeline: SDK lock seeds the shared closure at
    // the SDK's exact versions, then the SDK manifest and per-edge
    // plan are generated against the resolved graph.
    std::fs::copy(&sdk.lockfile, spike_dir.join("Cargo.lock")).expect("seed the spike lock");
    let manifest =
        SdkManifest::generate_dev(&workspace_root(), &sdk).expect("generate the SDK manifest");
    assert!(
        manifest.artifact("bevy_math", "0.19.0").is_some() || manifest.len() > 100,
        "manifest looks empty or truncated ({} entries)",
        manifest.len()
    );
    let edges = write_plan(&spike_dir, &manifest, &map_path).expect("write the redirect plan");
    println!("redirect plan: {edges} edges");
    let plan_text = std::fs::read_to_string(&map_path).unwrap();
    assert!(
        plan_text.contains(":glam="),
        "no glam edge redirected; the plan is not covering ecosystem deps"
    );

    // Cargo does not fingerprint the wrapper's behavior: units built
    // under an older wrapper or plan stay "fresh" and poison the run.
    // Always build from clean. (The production pipeline salts its
    // target dir instead.)
    let _ = std::fs::remove_dir_all(&spike_target);

    // Build the spike project: whole-graph redirect, plan supplied.
    // The crate type is a build flag, never a manifest entry: the Rust
    // dylib keeps the .rustc metadata section (linkage identity) that
    // a cdylib would strip.
    let status = Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &triple])
        .current_dir(&spike_dir)
        .env("CARGO_TARGET_DIR", &spike_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_SDK_EXTERN_MAP", &map_path)
        .env("JACKDAW_WRAPPER_LOG", "1")
        .status()
        .expect("spawn cargo for the spike project");
    assert!(
        status.success(),
        "spike project failed to build: the whole-graph redirect is incoherent"
    );

    let spike_dylib = spike_target.join(format!(
        "{triple}/debug/{}spike_game2{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(spike_dylib.exists(), "spike dylib missing");

    // Negative control.
    let mut before = TypeRegistry::default();
    before.register_derived_types();
    assert!(
        before.get_with_type_path(SPIKE_TYPE_PATH).is_none(),
        "spike type visible before dlopen"
    );

    let lib = unsafe { libloading::Library::new(&spike_dylib) }.expect("dlopen the spike dylib");
    std::mem::forget(lib);

    let mut after = TypeRegistry::default();
    after.register_derived_types();

    // The user's component crossed the boundary.
    let registration = after
        .get_with_type_path(SPIKE_TYPE_PATH)
        .expect("SPIKE FAILED: user component not registered after dlopen");
    assert!(
        registration
            .data::<bevy::ecs::reflect::ReflectComponent>()
            .is_some(),
        "registration lacks ReflectComponent data"
    );

    // avian's own reflected types crossed too: the ecosystem crate was
    // compiled with the same shared bevy_reflect, so its derives submitted
    // into the same inventory.
    let avian_types = after
        .iter()
        .filter(|reg| reg.type_info().type_path().starts_with("avian3d::"))
        .count();
    assert!(
        avian_types > 0,
        "no avian3d types registered; ecosystem reflection did not cross"
    );

    println!(
        "SPIKE PASSED: {SPIKE_TYPE_PATH} + {avian_types} avian3d types registered across dlopen"
    );
}
