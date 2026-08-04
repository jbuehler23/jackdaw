#![expect(clippy::print_stdout, reason = "test prints progress diagnostics")]
//! Probe: a built game binary reports its reflected types when run with
//! `--jackdaw-extract-schema`, so the editor can learn project components
//! without mapping project code into its own process.
//!
//! Builds `tests/fixtures/reflect_game` as an ordinary cargo binary and
//! checks the emitted JSON contains the component with its fields and a
//! default value.
//!
//! ```text
//! cargo test --test schema_extract -- --nocapture
//! ```

use std::path::PathBuf;

use jackdaw_project_build::shim::ShimSpec;
use jackdaw_project_build::{BuildEvent, build_project_binary};
use jackdaw_schema::ProjectSchema;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn extractor_dumps_project_component_schema() {
    let fixture_dir = workspace_root().join("tests/fixtures/reflect_game");
    let jackdaw_dir = workspace_root().join("target/schema_extract_reflect");
    let _ = std::fs::remove_dir_all(&jackdaw_dir);
    std::fs::create_dir_all(&jackdaw_dir).expect("create build dir");

    let spec = ShimSpec {
        package_name: "reflect_game".into(),
        crate_name: "reflect_game".into(),
        project_root: fixture_dir,
        extension_type: None,
    };
    let mut ignore = |_: BuildEvent| {};
    let build = build_project_binary(&spec, &jackdaw_dir, &mut ignore)
        .expect("build the reflect_game binary");
    let schema = build
        .schema
        .expect("schema extracted from the built binary");

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
        ["strength", "label"],
        "fields should match the Reflect derive"
    );
    assert!(
        component.default.is_some(),
        "ReflectDefault should produce a default value for the picker"
    );

    // Round-trip through the on-disk file the editor watches.
    let on_disk: ProjectSchema =
        jackdaw_schema::read_schema(&jackdaw_dir).expect("schema.json written");
    assert!(
        on_disk
            .components
            .iter()
            .any(|c| c.type_path == "reflect_game::AutoRegisteredComponent")
    );
}
