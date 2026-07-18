//! `cargo metadata` parsing: a project's package and lib crate names,
//! read without any editor or bevy dependency so the build pipeline and
//! the CLI can resolve a project from the filesystem alone.

use std::path::Path;

use serde::Deserialize;

/// A project's cargo metadata: packages and their targets.
#[derive(Deserialize)]
pub struct CargoMeta {
    packages: Vec<MetaPackage>,
}

#[derive(Deserialize)]
struct MetaPackage {
    name: String,
    targets: Vec<MetaTarget>,
}

#[derive(Deserialize)]
struct MetaTarget {
    name: String,
    kind: Vec<String>,
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

    /// The project's lib crate name (underscored), the name the generated
    /// shim imports.
    pub fn lib_name(&self) -> Option<String> {
        self.lib_package().map(|(_, crate_name)| crate_name)
    }

    /// The first package that has a library target, as
    /// `(package_name, crate_name)`. The package name keeps its dashes
    /// (the shim's path dep keys on it); the crate name is underscored
    /// (code references the crate by it).
    pub fn lib_package(&self) -> Option<(String, String)> {
        self.packages.iter().find_map(|p| {
            p.targets
                .iter()
                .find(|t| t.kind.iter().any(|k| k == "lib" || k == "rlib"))
                .map(|t| (p.name.clone(), t.name.replace('-', "_")))
        })
    }
}
