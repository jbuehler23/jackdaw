//! Project run configurations read from `jackdaw.toml`.

use std::path::Path;

use bevy::prelude::*;
use jackdaw_pie_protocol::manifest::{Manifest, RunConfig};

/// The open project's run configurations. Empty when the project has no
/// `jackdaw.toml` and no single default could be synthesized.
#[derive(Resource, Default)]
pub struct RunConfigs {
    pub manifest: Manifest,
}

/// Read `<root>/jackdaw.toml` when a project opens. A missing file or a
/// parse error yields an empty manifest rather than failing the open;
/// a single-binary project gets a synthesized default (added later).
pub fn read_run_configs(world: &mut World) {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        return;
    };
    let manifest = load_manifest(&root);
    world.insert_resource(RunConfigs { manifest });
}

fn load_manifest(root: &Path) -> Manifest {
    let path = root.join("jackdaw.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_manifest_str(&text),
        Err(_) => synthesize_default(root),
    }
}

/// Parse a manifest string, logging and falling back to empty on error.
fn parse_manifest_str(text: &str) -> Manifest {
    match toml::from_str::<Manifest>(text) {
        Ok(m) => m,
        Err(err) => {
            warn!("jackdaw.toml parse error: {err}");
            Manifest::default()
        }
    }
}

/// `cargo metadata` parsing moved to the bevy-light build-pipeline crate
/// so the CLI can resolve a project without the editor. Re-exported here
/// at its historical path.
pub use jackdaw_project_build::cargo_meta::CargoMeta;

/// A config-less project gets a one-entry default so its Play button
/// works with no manifest: every run launches the same project dylib
/// through the game runner, so there is nothing to resolve.
fn synthesize_default(_root: &Path) -> Manifest {
    Manifest {
        plugin: None,
        runs: vec![RunConfig::default()],
    }
}

/// Produce a starter `jackdaw.toml` body: the plugin override slot, the
/// version pins the open check compares against, and one default run.
pub fn scaffold_manifest(_meta: &CargoMeta) -> String {
    format!(
        "# The game plugin type inside your lib crate. Uncomment to\n\
         # override source detection.\n\
         # plugin = \"GamePlugin\"\n\
         \n\
         {pins}\n\
         [[run]]\n\
         name = \"Play\"\n",
        pins = jackdaw_project_build::project_manifest::pins_toml()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest_string() {
        let m = parse_manifest_str(
            r#"[[run]]
name = "Server""#,
        );
        assert_eq!(m.runs.len(), 1);
        assert_eq!(m.runs[0].label(), "Server");
    }

    #[test]
    fn bad_toml_yields_empty_manifest() {
        let m = parse_manifest_str("this is not = valid = toml [[");
        assert!(m.runs.is_empty());
    }

    const META_FIXTURE: &str = r#"{
      "packages": [
        {"name":"my-game","targets":[
          {"name":"my_game","kind":["lib"]},
          {"name":"my-game","kind":["bin"]}
        ]}
      ]
    }"#;

    #[test]
    fn finds_the_lib_name() {
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();
        assert_eq!(meta.lib_name().as_deref(), Some("my_game"));
    }

    #[test]
    fn scaffold_has_a_default_run() {
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();
        let body = scaffold_manifest(&meta);
        assert!(body.contains("[[run]]"));
        assert!(body.contains("# plugin"));
    }
}
