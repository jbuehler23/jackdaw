//! Probe: the out-of-process schema extractor produces a usable
//! project schema, so the editor can learn a project's component types
//! without mapping project code into its own process.
//!
//! Builds `.scratch/project-onboarding/spike1/spike_game` (a plain
//! Bevy dylib deriving `Reflect` on one component) and the
//! `jackdaw-runner` binary, runs `jackdaw-runner --extract-schema` on
//! the dylib, and checks the emitted JSON contains the component with
//! its fields and a default value.
//!
//! ```text
//! cargo test --features "dylib runner" --target <host-triple> \
//!     --test spike_schema_extract -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use jackdaw::project_build::schema::ProjectSchema;
use jackdaw::sdk_paths::SdkPaths;

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
        .args([
            "build",
            "-p",
            "jackdaw",
            "--bin",
            "jackdaw-runner",
            "--features",
            "runner",
            "--target",
            &triple,
        ])
        .current_dir(workspace_root())
        .status()
        .expect("build jackdaw-runner");
    assert!(status.success(), "runner build failed");

    // Build the spike project as a Rust dylib through the SDK pipeline.
    let spike_dir = workspace_root().join(".scratch/project-onboarding/spike1/spike_game");
    let spike_target = spike_dir.join("target-spike");
    let status = Command::new("cargo")
        .args(["rustc", "--crate-type", "dylib", "--target", &triple])
        .current_dir(&spike_dir)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &spike_target)
        .env("RUSTC_WRAPPER", &sdk.wrapper)
        .env("JACKDAW_SDK_DYLIB", &sdk.dylib)
        .env("JACKDAW_SDK_DEPS", &sdk.deps)
        .env("JACKDAW_SDK_HOST_DEPS", &sdk.host_deps)
        .status()
        .expect("build the spike dylib");
    assert!(status.success(), "spike dylib failed to build");
    let dylib = spike_target.join(format!(
        "{triple}/debug/{}spike_game{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(dylib.exists(), "spike dylib missing");

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

    let spike = schema
        .components
        .iter()
        .find(|c| c.type_path == "spike_game::SpikeAutoComponent")
        .expect("SpikeAutoComponent must be in the extracted schema");

    assert_eq!(spike.short_name, "SpikeAutoComponent");
    let field_names: Vec<&str> = spike.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        field_names,
        vec!["strength", "label"],
        "the component's fields must be captured"
    );
    let field_types: Vec<&str> = spike.fields.iter().map(|f| f.type_path.as_str()).collect();
    assert_eq!(field_types, vec!["f32", "alloc::string::String"]);
    assert!(
        spike.default.is_some() && spike.default_constructible,
        "the component derives Default, so a default value must be captured"
    );

    println!(
        "SPIKE PASSED: extractor produced the schema for {} ({:?}), default = {:?}",
        spike.type_path, field_names, spike.default
    );
}
