//! Project run configurations read from `jackdaw.toml`.

use std::path::Path;

use bevy::prelude::*;
use jackdaw_pie_protocol::manifest::{Manifest, RunConfig};
use serde::Deserialize;

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

/// Trimmed `cargo metadata --no-deps` output: workspace packages with
/// their bin targets and direct dependencies.
#[derive(Deserialize)]
pub struct CargoMeta {
    packages: Vec<MetaPackage>,
}

#[derive(Deserialize)]
struct MetaPackage {
    name: String,
    targets: Vec<MetaTarget>,
    dependencies: Vec<MetaDep>,
}

#[derive(Deserialize)]
struct MetaTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Deserialize)]
struct MetaDep {
    name: String,
}

/// A buildable binary target and the package that owns it.
pub struct BinTarget {
    pub bin: String,
    pub package: String,
}

impl CargoMeta {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Run `cargo metadata --no-deps` in `project_dir`.
    pub fn load(project_dir: &Path) -> Option<Self> {
        let out = std::process::Command::new("cargo")
            .current_dir(project_dir)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Self::parse(&String::from_utf8_lossy(out.stdout.as_slice())).ok()
    }

    pub fn bins(&self) -> Vec<BinTarget> {
        let mut out = Vec::new();
        for p in &self.packages {
            for t in &p.targets {
                if t.kind.iter().any(|k| k == "bin") {
                    out.push(BinTarget {
                        bin: t.name.clone(),
                        package: p.name.clone(),
                    });
                }
            }
        }
        out
    }

    pub fn package_depends_directly(&self, package: &str, dep: &str) -> bool {
        self.packages
            .iter()
            .find(|p| p.name == package)
            .is_some_and(|p| p.dependencies.iter().any(|d| d.name == dep))
    }

    pub fn package_of_bin(&self, bin: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|p| {
                p.targets
                    .iter()
                    .any(|t| t.name == bin && t.kind.iter().any(|k| k == "bin"))
            })
            .map(|p| p.name.as_str())
    }
}

/// Resolve the build inputs for a run config: its owning package, plus
/// the features that enable PIE. `jackdaw_runtime/pie` is auto-added
/// when the bin's package depends on `jackdaw_runtime` directly;
/// otherwise the config's own `features` must supply the path.
pub fn resolve_build_spec(
    meta: &CargoMeta,
    run: &RunConfig,
) -> Option<crate::ext_build::BuildSpec> {
    let package = meta.package_of_bin(&run.bin)?.to_string();
    let mut features = run.features.clone();
    if meta.package_depends_directly(&package, "jackdaw_runtime") {
        features.push("jackdaw_runtime/pie".to_string());
    }
    Some(crate::ext_build::BuildSpec {
        package,
        bin: run.bin.clone(),
        features,
    })
}

/// A config-less project with exactly one bin gets a one-entry default
/// so its Play button works with no manifest. Zero or many bins yields
/// an empty manifest (the UI offers the scaffold instead).
fn synthesize_default(root: &Path) -> Manifest {
    let Some(meta) = CargoMeta::load(root) else {
        return Manifest::default();
    };
    let bins = meta.bins();
    if bins.len() == 1 {
        Manifest {
            runs: vec![default_run(&bins[0].bin)],
        }
    } else {
        Manifest::default()
    }
}

fn default_run(bin: &str) -> RunConfig {
    RunConfig {
        bin: bin.to_string(),
        name: None,
        instances: 1,
        features: Vec::new(),
        env: Default::default(),
        args: Vec::new(),
        cwd: None,
        mode: Default::default(),
    }
}

/// Produce a starter `jackdaw.toml` body listing one `[[run]]` per bin,
/// flagging any bin that lacks a direct `jackdaw_runtime` dependency.
pub fn scaffold_manifest(meta: &CargoMeta) -> String {
    let mut out = String::new();
    for b in meta.bins() {
        out.push_str("[[run]]\n");
        out.push_str(&format!("bin = \"{}\"\n", b.bin));
        if !meta.package_depends_directly(&b.package, "jackdaw_runtime") {
            out.push_str("# This bin depends on jackdaw_runtime only indirectly.\n");
            out.push_str("# Add a feature that enables jackdaw_runtime/pie and list it:\n");
            out.push_str("# features = [\"<crate>/pie\"]\n");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest_string() {
        let m = parse_manifest_str(
            r#"[[run]]
bin = "srv"
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
        {"name":"srv","targets":[{"name":"srv","kind":["bin"]}],
         "dependencies":[{"name":"jackdaw_runtime"}]},
        {"name":"cli","targets":[{"name":"cli","kind":["bin"]}],
         "dependencies":[{"name":"world"}]}
      ]
    }"#;

    #[test]
    fn enumerates_bins() {
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();
        let bins = meta.bins();
        assert!(bins.iter().any(|b| b.bin == "srv" && b.package == "srv"));
        assert!(bins.iter().any(|b| b.bin == "cli"));
    }

    #[test]
    fn detects_direct_runtime_dep() {
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();
        assert!(meta.package_depends_directly("srv", "jackdaw_runtime"));
        assert!(!meta.package_depends_directly("cli", "jackdaw_runtime"));
    }

    #[test]
    fn resolves_features_direct_vs_transitive() {
        use jackdaw_pie_protocol::manifest::RunConfig;
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();

        let srv = RunConfig {
            bin: "srv".into(),
            name: None,
            instances: 1,
            features: vec![],
            env: Default::default(),
            args: vec![],
            cwd: None,
            mode: Default::default(),
        };
        let spec = resolve_build_spec(&meta, &srv).unwrap();
        assert_eq!(spec.package, "srv");
        assert!(spec.features.contains(&"jackdaw_runtime/pie".to_string()));

        let cli = RunConfig {
            bin: "cli".into(),
            name: None,
            instances: 1,
            features: vec!["world/pie".into()],
            env: Default::default(),
            args: vec![],
            cwd: None,
            mode: Default::default(),
        };
        let spec = resolve_build_spec(&meta, &cli).unwrap();
        assert_eq!(spec.features, vec!["world/pie".to_string()]);
    }

    #[test]
    fn scaffold_flags_transitive_bins() {
        let meta = CargoMeta::parse(META_FIXTURE).unwrap();
        let body = scaffold_manifest(&meta);
        // The transitively-dependent bin is flagged; the direct one is not.
        let cli_idx = body.find("bin = \"cli\"").unwrap();
        let srv_idx = body.find("bin = \"srv\"").unwrap();
        assert!(body[cli_idx..].contains("jackdaw_runtime only indirectly"));
        let srv_section_end = body[srv_idx..]
            .find("[[run]]")
            .map(|i| srv_idx + i)
            .unwrap_or(body.len());
        assert!(!body[srv_idx..srv_section_end].contains("indirectly"));
    }
}
