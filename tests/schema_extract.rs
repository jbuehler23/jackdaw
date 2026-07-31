#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Probe: the out-of-process schema extractor produces a usable
//! project schema, so the editor can learn a project's component types
//! without mapping project code into its own process.
//!
//! Builds `tests/fixtures/reflect_game` (a plain Bevy library deriving
//! `Reflect` on one component) through the real project pipeline, then
//! runs `jackdaw-runner --extract-schema` on the result and checks the
//! emitted JSON contains the component with its fields and a default
//! value.
//!
//! The pipeline is what makes this work: `jackdaw_extract_schema` is an
//! export the generated shim provides, never the user's crate. Building
//! the fixture directly produced a dylib without it, and the extractor
//! failed on `undefined symbol: jackdaw_extract_schema`.
//!
//! ```text
//! cargo test --features "dylib runner" --target <host-triple> \
//!     --test schema_extract -- --nocapture
//! ```
#![cfg(feature = "dylib")]

use std::path::PathBuf;
use std::process::Command;

use jackdaw::project_build::schema::ProjectSchema;
use jackdaw::project_build::{BuildEvent, build_project_dylib, shim_spec_for_project};
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

    // Build the fixture the way the editor builds a project: through
    // the shim, which is what supplies the `jackdaw_extract_schema`
    // export the extractor looks for.
    let fixture_dir = util::stage_fixture("reflect_game");
    let spec = shim_spec_for_project(&fixture_dir, None).expect("the fixture is a lib crate");
    let jackdaw_dir = fixture_dir.join(".jackdaw");
    let mut ignore_progress = |_: BuildEvent| {};
    let build = build_project_dylib(
        &spec,
        &jackdaw_dir,
        &sdk,
        Some(&workspace_root()),
        &mut ignore_progress,
    )
    .expect("build the fixture dylib through the pipeline");
    let dylib = build.dylib.clone();

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
