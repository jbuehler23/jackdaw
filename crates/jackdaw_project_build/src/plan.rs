//! The per-edge extern redirect plan and the lock alignment that
//! precedes it.
//!
//! A project builds against the shipped SDK by having the rustc
//! wrapper rewrite `--extern` flags to the SDK's exact artifacts. The
//! decisions are per dependency edge: an edge redirects only when the
//! project resolves that dependency at the byte-identical version the
//! SDK holds. Name-keyed redirection cannot work (the SDK closure
//! itself holds two hashbrown versions; user graphs hold private newer
//! copies of closure crates), so the plan is a `consumer:alias=artifact`
//! line per edge, consumed by `jackdaw-rustc-wrapper`.
//!
//! Before planning, the project's lockfile is aligned: closure crates
//! resolved at a semver-compatible but different version get pinned to
//! the SDK's exact version. The lockfile in question is the generated
//! shim crate's, never the user's.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::sdk_paths::SdkPaths;

#[derive(Debug)]
pub enum PlanError {
    Io(std::io::Error),
    Cargo(String),
    Parse(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Cargo(msg) => write!(f, "cargo failed: {msg}"),
            Self::Parse(msg) => write!(f, "could not parse: {msg}"),
        }
    }
}

impl std::error::Error for PlanError {}

impl From<std::io::Error> for PlanError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The SDK's runtime closure with the exact artifact each crate
/// compiled to: `(name, version) -> artifact path`. Loaded from the
/// shipped `manifest.txt` in installed layouts, generated from the
/// workspace build in dev.
pub struct SdkManifest {
    artifacts: BTreeMap<(String, String), String>,
}

impl SdkManifest {
    /// Load `name version artifact` lines.
    pub fn load(path: &Path) -> Result<Self, PlanError> {
        let contents = std::fs::read_to_string(path)?;
        let mut artifacts = BTreeMap::new();
        for line in contents.lines() {
            let mut parts = line.splitn(3, ' ');
            let (Some(name), Some(version), Some(artifact)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(PlanError::Parse(format!("bad manifest line: {line}")));
            };
            artifacts.insert(
                (name.to_string(), version.to_string()),
                artifact.to_string(),
            );
        }
        Ok(Self { artifacts })
    }

    /// Generate the manifest from a dev workspace: the SDK's runtime
    /// closure (`cargo tree -e normal,no-proc-macro`, which keeps
    /// proc-macro crates and their host-side support libraries out)
    /// joined with the workspace build's artifact list. Requires the
    /// SDK to have been built with `--features dylib --target <triple>`;
    /// the triple-dir filter is what separates target-side artifacts
    /// from host-side units of the same crate. The result is written to
    /// `sdk.manifest` so later opens skip the cargo runs.
    pub fn generate_dev(workspace_root: &Path, sdk: &SdkPaths) -> Result<Self, PlanError> {
        // Enumerate artifacts from the same profile the SDK was built at, so a
        // release SDK's manifest points at release rlibs (what projects redirect
        // against) rather than debug ones from a stray earlier build.
        let mut args = vec!["-p", "jackdaw", "--features", "dylib"];
        let is_release = sdk
            .dylib
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|name| name == "release");
        if is_release {
            args.push("--release");
        }
        Self::generate(workspace_root, sdk, &args)
    }

    /// Enumerate the SDK's runtime-closure artifacts by building
    /// `build_args` and reading cargo's JSON. The dev workspace builds the
    /// editor (`-p jackdaw --features dylib`); the bootstrap recipe, which
    /// has no editor package, builds the SDK crates directly
    /// (`-p jackdaw_sdk -p jackdaw_runner --release`). The SDK is already
    /// compiled, so this re-invocation just reports the (fresh) artifact
    /// filenames.
    ///
    /// `build_args` MUST name the same package set the SDK was built with.
    /// A narrower set resolves different features for shared dependencies
    /// (bevy) and turns this enumeration into a full second rebuild instead
    /// of a cache hit.
    pub fn generate(
        workspace_root: &Path,
        sdk: &SdkPaths,
        build_args: &[&str],
    ) -> Result<Self, PlanError> {
        let closure = sdk_runtime_closure(workspace_root)?;

        // Capture stdout (the JSON artifact stream) but let cargo's own
        // progress reach the terminal, so this step never looks hung on a
        // cold cache.
        let child = Command::new("cargo")
            .arg("build")
            .args(build_args)
            .args(["--target", &sdk.triple, "--message-format=json"])
            .current_dir(workspace_root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(PlanError::Cargo(
                "SDK build for manifest generation failed".into(),
            ));
        }

        let triple_dir = format!("/{}/", sdk.triple);
        let mut artifacts = BTreeMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if msg["reason"] != "compiler-artifact" {
                continue;
            }
            let Some(name) = msg["target"]["name"].as_str() else {
                continue;
            };
            let name = name.replace('-', "_");
            let Some(version) = msg["package_id"].as_str().and_then(package_id_version) else {
                continue;
            };
            if !closure.contains(&(name.clone(), version.to_string())) {
                continue;
            }
            let kinds = msg["target"]["kind"]
                .as_array()
                .map(|k| k.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            if kinds.contains(&"proc-macro") || kinds.contains(&"custom-build") {
                continue;
            }
            let Some(filenames) = msg["filenames"].as_array() else {
                continue;
            };
            // Prefer the rlib: the final dylib link needs code, and
            // rustc dedupes a same-SVH crate already linked into the
            // SDK dylib instead of embedding the rlib a second time.
            // Only accept artifacts from the triple dir.
            let artifact = filenames
                .iter()
                .filter_map(|f| f.as_str())
                .filter(|f| f.contains(&triple_dir))
                .find(|f| f.ends_with(".rlib"))
                .or_else(|| {
                    filenames
                        .iter()
                        .filter_map(|f| f.as_str())
                        .filter(|f| f.contains(&triple_dir))
                        .find(|f| f.ends_with(".rmeta"))
                });
            if let Some(artifact) = artifact {
                artifacts.insert((name, version.to_string()), artifact.to_string());
            }
        }

        let manifest = Self { artifacts };
        manifest.write(&sdk.manifest)?;
        Ok(manifest)
    }

    pub fn write(&self, path: &Path) -> Result<(), PlanError> {
        let mut file = std::fs::File::create(path)?;
        for ((name, version), artifact) in &self.artifacts {
            writeln!(file, "{name} {version} {artifact}")?;
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    /// Manifest entries whose artifact is no longer on disk, as
    /// `name version` pairs, capped at `limit`.
    ///
    /// A manifest is a cache of one build's filenames. Cargo replaces
    /// those on every rebuild and prunes what it no longer needs, so
    /// entries rot without anything noticing until rustc is handed a
    /// path that does not resolve. Checking costs one stat per entry.
    /// Only meaningful for absolute paths: a shipped manifest stores
    /// basenames that are rebased onto the install at use time.
    pub fn missing_artifacts(&self, limit: usize) -> Vec<String> {
        self.artifacts
            .iter()
            .filter(|(_, artifact)| {
                let path = Path::new(artifact.as_str());
                path.is_absolute() && !path.exists()
            })
            .take(limit)
            .map(|((name, version), _)| format!("{name} {version}"))
            .collect()
    }

    pub fn artifact(&self, name: &str, version: &str) -> Option<&str> {
        self.artifacts
            .get(&(name.to_string(), version.to_string()))
            .map(String::as_str)
    }

    /// How many versions of a crate the SDK closure holds. A graph
    /// legitimately carries several majors of one crate (two `rand`s),
    /// each compiling to its own artifact, so this is what separates an
    /// expected pair of artifacts from a crate built twice over.
    pub fn version_count(&self, name: &str) -> usize {
        self.artifacts
            .keys()
            .filter(|(crate_name, _)| crate_name == name)
            .count()
    }

    /// The artifact for a crate by name, ignoring version. The SDK closure
    /// holds a single bevy and `jackdaw_api`, so this uniquely resolves the
    /// rlibs the static wrapper points the bevy facade and the `jackdaw_api`
    /// injection at.
    pub fn artifact_for(&self, name: &str) -> Option<&str> {
        self.artifacts
            .iter()
            .find(|((n, _), _)| n == name)
            .map(|(_, artifact)| artifact.as_str())
    }
}

/// Resolve a manifest artifact reference to an absolute path the rustc
/// wrapper can hand to `--extern`. Dev and bootstrap manifests store
/// absolute paths (used verbatim); a shipped SDK's manifest stores bare
/// basenames, which the loader has no fixed root for, so they are joined
/// with the install's `deps/` dir here. Keeping the shipped form
/// location-independent is what lets a downloaded SDK build projects
/// wherever it is unpacked, without rewriting the manifest on install.
fn resolve_artifact(artifact: &str, deps_dir: &Path) -> String {
    let path = Path::new(artifact);
    if path.is_absolute() {
        artifact.to_string()
    } else {
        deps_dir.join(artifact).to_string_lossy().into_owned()
    }
}

/// Write the per-edge redirect plan for the build root's resolve
/// graph: `consumer:alias=artifact` lines, one per dependency edge
/// whose resolved version is byte-identical to the SDK's. Consumed by
/// the rustc wrapper via `JACKDAW_SDK_EXTERN_MAP`. `deps_dir` is the
/// SDK's `deps/` directory, used to resolve basename-only artifacts in a
/// shipped manifest. Returns the number of edges written.
pub fn write_plan(
    build_root: &Path,
    manifest: &SdkManifest,
    deps_dir: &Path,
    out_path: &Path,
) -> Result<usize, PlanError> {
    let metadata = cargo_metadata(build_root)?;
    let mut contents = Vec::new();
    let mut edges = 0;
    let empty = Vec::new();
    for node in metadata["resolve"]["nodes"].as_array().unwrap_or(&empty) {
        let Some(consumer_id) = node["id"].as_str() else {
            continue;
        };
        let (Some(consumer), Some(consumer_version)) = (
            package_id_name(consumer_id),
            package_id_version(consumer_id),
        ) else {
            continue;
        };
        for dep in node["deps"].as_array().unwrap_or(&empty) {
            let Some(alias) = dep["name"].as_str() else {
                continue;
            };
            let Some(pkg_id) = dep["pkg"].as_str() else {
                continue;
            };
            let (Some(dep_name), Some(dep_version)) =
                (package_id_name(pkg_id), package_id_version(pkg_id))
            else {
                continue;
            };
            if let Some(artifact) = manifest.artifact(&dep_name, dep_version) {
                // Key the edge on the consumer's exact version: a graph
                // can hold two versions of one crate (two `rand`s), each
                // wanting a different version of the same dependency, so
                // the name alone cannot pick the right redirect.
                let resolved = resolve_artifact(artifact, deps_dir);
                writeln!(contents, "{consumer}@{consumer_version}:{alias}={resolved}")?;
                edges += 1;
            }
        }
    }
    if std::fs::read(out_path).ok().as_deref() != Some(contents.as_slice()) {
        std::fs::write(out_path, contents)?;
    }
    Ok(edges)
}

/// The SDK dylib's runtime dependency closure: `(name, version)`
/// pairs from `cargo tree`. Dev-checkout only.
fn sdk_runtime_closure(workspace_root: &Path) -> Result<BTreeSet<(String, String)>, PlanError> {
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "jackdaw_sdk",
            "-e",
            "normal,no-proc-macro",
            "--prefix",
            "none",
        ])
        .current_dir(workspace_root)
        .output()?;
    if !output.status.success() {
        return Err(PlanError::Cargo("cargo tree failed".into()));
    }
    let mut closure = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(version)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Some(version) = version.strip_prefix('v') else {
            continue;
        };
        closure.insert((name.replace('-', "_"), version.to_string()));
    }
    Ok(closure)
}

fn cargo_metadata(dir: &Path) -> Result<serde_json::Value, PlanError> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(dir)
        .output()?;
    if !output.status.success() {
        return Err(PlanError::Cargo(format!(
            "cargo metadata failed in {}",
            dir.display()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| PlanError::Parse(format!("cargo metadata output: {e}")))
}

/// The version embedded in a cargo package id
/// (`registry+https://...#glam@0.32.1` or `path+file:///...#0.1.0`).
fn package_id_version(id: &str) -> Option<&str> {
    let fragment = id.rsplit_once('#')?.1;
    Some(match fragment.rsplit_once('@') {
        Some((_, version)) => version,
        None => fragment,
    })
}

/// The package name embedded in a cargo package id, normalized to
/// underscores. Registry ids carry it in the fragment; path ids carry
/// only a version there, so the name is the last path segment.
fn package_id_name(id: &str) -> Option<String> {
    package_id_raw_name(id).map(|n| n.replace('-', "_"))
}

/// The package name exactly as cargo spells it (`cargo update -p`
/// rejects normalized names).
fn package_id_raw_name(id: &str) -> Option<String> {
    let (base, fragment) = id.rsplit_once('#')?;
    let name = match fragment.rsplit_once('@') {
        Some((name, _)) => name,
        None => base.rsplit('/').next()?,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_ids_parse() {
        let reg = "registry+https://github.com/rust-lang/crates.io-index#glam@0.32.1";
        assert_eq!(package_id_version(reg), Some("0.32.1"));
        assert_eq!(package_id_name(reg).as_deref(), Some("glam"));
        let path = "path+file:///home/u/my-game#0.1.0";
        assert_eq!(package_id_version(path), Some("0.1.0"));
        assert_eq!(package_id_name(path).as_deref(), Some("my_game"));
        assert_eq!(package_id_raw_name(path).as_deref(), Some("my-game"));
    }

    #[test]
    fn manifest_round_trips() {
        let dir = std::env::temp_dir().join("jackdaw_manifest_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("manifest.txt");
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            ("glam".to_string(), "0.32.1".to_string()),
            "/sdk/deps/libglam-abc.rlib".to_string(),
        );
        let manifest = SdkManifest { artifacts };
        manifest.write(&path).unwrap();
        let back = SdkManifest::load(&path).unwrap();
        assert_eq!(
            back.artifact("glam", "0.32.1"),
            Some("/sdk/deps/libglam-abc.rlib")
        );
    }

    #[test]
    fn resolve_artifact_absolute_passthrough_basename_rebases() {
        let deps = Path::new("/opt/jackdaw/sdk/x86_64/deps");
        // Dev/bootstrap manifests store absolute paths: used verbatim.
        assert_eq!(
            resolve_artifact("/ws/target/x86_64/release/deps/libglam-abc.rlib", deps),
            "/ws/target/x86_64/release/deps/libglam-abc.rlib"
        );
        // A shipped manifest stores basenames, rebased onto the install deps.
        assert_eq!(
            resolve_artifact("libglam-abc.rlib", deps),
            "/opt/jackdaw/sdk/x86_64/deps/libglam-abc.rlib"
        );
    }
}
