#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Probe: the out-of-process schema extractor produces a usable
//! project schema, so the editor can learn a project's component types
//! without mapping project code into its own process.
//!
//! Builds `tests/fixtures/reflect_game` (a plain
//! Bevy dylib deriving `Reflect` on one component) and the
//! `jackdaw-runner` binary, runs `jackdaw-runner --extract-schema` on
//! the dylib, and checks the emitted JSON contains the component with
//! its fields and a default value.
//!
//! ```text
//! cargo test --features "dylib runner" --target <host-triple> \
//!     --test schema_extract -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use jackdaw::project_build::schema::ProjectSchema;
use jackdaw::sdk_paths::SdkPaths;

mod util;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn extractor_dumps_project_component_schema() {
    let sdk = SdkPaths::for_workspace(&workspace_root());
    let triple = sdk.triple.clone();
    assert!(
        sdk.dylib_exists(),
        "SDK dylib missing; build with --features dylib --target {triple}"
    );

    // Build the runner (prebuilt in production; here it shares the
    // cached graph).
    let status = Command::new("cargo")
        .args(["build", "-p", "jackdaw_runner", "--target", &triple])
        .current_dir(workspace_root())
        .status()
        .expect("build jackdaw-runner");
    assert!(status.success(), "runner build failed");

    // Build the fixture project as a Rust dylib through the SDK pipeline.
    let fixture_dir = util::stage_fixture("reflect_game");
    let fixture_target = fixture_dir.join("target-fixture");
    let status = Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &triple])
        .current_dir(&fixture_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &fixture_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .status()
        .expect("build the fixture dylib");
    assert!(status.success(), "fixture dylib failed to build");
    let dylib = fixture_target.join(format!(
        "{triple}/debug/{}reflect_game{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(dylib.exists(), "fixture dylib missing");

    // Run the extractor.
    let runner = sdk.runner.clone();
    let output = Command::new(&runner)
        .arg("--extract-schema")
        .arg(&dylib)
        .output()
        .expect("run the extractor");
    assert!(
        output.status.success(),
        "extractor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let schema: ProjectSchema =
        serde_json::from_slice(&output.stdout).expect("parse the extractor's JSON");
    println!(
        "extracted {} component types, {} resource types",
        schema.components.len(),
        schema.resources.len()
    );

    let component = schema
        .components
        .iter()
        .find(|c| c.type_path == "reflect_game::AutoRegisteredComponent")
        .expect("AutoRegisteredComponent must be in the extracted schema");

    assert_eq!(component.short_name, "AutoRegisteredComponent");
    let field_names: Vec<&str> = component.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        field_names,
        vec!["strength", "label"],
        "the component's fields must be captured"
    );
    let field_types: Vec<&str> = component
        .fields
        .iter()
        .map(|f| f.type_path.as_str())
        .collect();
    assert_eq!(field_types, vec!["f32", "alloc::string::String"]);
    assert!(
        component.default.is_some() && component.default_constructible,
        "the component derives Default, so a default value must be captured"
    );

    println!(
        "extractor produced the schema for {} ({:?}), default = {:?}",
        component.type_path, field_names, component.default
    );
}
