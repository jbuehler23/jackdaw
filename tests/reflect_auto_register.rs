#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! `reflect_auto_register` across `dlopen` with the shared SDK.
//!
//! Builds `tests/fixtures/reflect_game` (a plain Bevy library deriving
//! `Reflect` on one component, with NO registration code of its own)
//! through the SDK pipeline, with the shim's generated
//! `jackdaw_register_types` entry appended, dlopens it, and checks that the
//! loader's handoff puts the component into a registry the host owns.
//!
//! Requires the `dylib` feature and an explicit `--target` (the host
//! triple): the test binary must link the same triple-dir
//! `libjackdaw_sdk` the fixture dylib links.
//!
//! ```text
//! cargo test --profile dev --features dylib --target <host-triple> \
//!     --test reflect_auto_register -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use bevy::reflect::TypeRegistry;
use jackdaw::project_build::plan::{SdkManifest, write_plan};
use jackdaw::project_build::shim::register_types_source;
use jackdaw::sdk_paths::SdkPaths;
use jackdaw_loader::{TypeHandoff, register_library_types};

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

    let fixture_dir = util::stage_fixture("reflect_game");
    let fixture_target = fixture_dir.join("target-fixture");
    let map_path = fixture_dir.join("extern_map.txt");

    // The editor's shim adds this entry to every library it builds.
    let lib_rs = fixture_dir.join("src/lib.rs");
    let mut source = std::fs::read_to_string(&lib_rs).expect("read the fixture source");
    source.push_str(register_types_source());
    std::fs::write(&lib_rs, source).expect("append the register-types entry");

    std::fs::copy(&sdk.lockfile, fixture_dir.join("Cargo.lock")).expect("seed the fixture lock");
    // Redirect against the compilation this test binary links, which is the
    // one the editor's own build produces in production. A plain
    // `-p jackdaw` selection leaves out dev-dependencies, and where those
    // add features to a shared crate (`tempfile` turns on `rustix/default`
    // on macOS) it resolves a second Bevy whose type ids differ from ours.
    let manifest = SdkManifest::generate(
        &workspace_root(),
        &sdk,
        &[
            "-p",
            "jackdaw",
            "--features",
            "dylib",
            "--profile",
            "dev",
            "--test",
            env!("CARGO_CRATE_NAME"),
        ],
    )
    .expect("generate the SDK manifest");
    write_plan(&fixture_dir, &manifest, &sdk.deps, &map_path).expect("write the redirect plan");

    // Wrapper behavior is not part of cargo's fingerprint; build from
    // clean so stale units cannot poison the probe.
    let _ = std::fs::remove_dir_all(&fixture_target);

    // Build the fixture project through the SDK pipeline, isolated target
    // dir. The crate type is a build flag, never a manifest entry: the
    // Rust dylib keeps the .rustc metadata section carrying the linkage
    // identity, which a cdylib would strip.
    let status = Command::new("cargo")
        .args([
            "rustc",
            "--lib",
            "--crate-type",
            "dylib",
            "--target",
            &triple,
        ])
        .current_dir(&fixture_dir)
        .env("CARGO_TARGET_DIR", &fixture_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .env("JACKDAW_SDK_EXTERN_MAP", &map_path)
        .env("JACKDAW_WRAPPER_LOG", "1")
        .status()
        .expect("spawn cargo for the fixture project");
    assert!(status.success(), "fixture project failed to build");

    let fixture_dylib = fixture_target.join(format!(
        "{triple}/debug/{}reflect_game{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
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

    let lib =
        unsafe { libloading::Library::new(&fixture_dylib) }.expect("dlopen the fixture dylib");

    let mut after = TypeRegistry::empty();
    let handoff = register_library_types(&lib, &fixture_dylib, &mut after);
    // Never unloaded; leak deliberately, mirroring the loader's rule.
    std::mem::forget(lib);
    assert_eq!(
        handoff,
        TypeHandoff::Entry,
        "the fixture dylib does not export `jackdaw_register_types`"
    );
    let registration = after.get_with_type_path(COMPONENT_TYPE_PATH);
    assert!(
        registration.is_some(),
        "{COMPONENT_TYPE_PATH} not in the registry after the library's \
         register-types entry ran"
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

    println!("{COMPONENT_TYPE_PATH} registered through the library entry");
}
