//! The per-edge extern redirect plan and the lock alignment that
//! precedes it.
//!
//! An extension builds against the shipped SDK by having the rustc
//! wrapper rewrite `--extern` flags to the SDK's exact artifacts. The
//! decisions are per dependency edge: an edge redirects only when the
//! extension resolves that dependency at the byte-identical version the
//! SDK holds. Name-keyed redirection cannot work (the SDK closure
//! itself holds two hashbrown versions; user graphs hold private newer
//! copies of closure crates), so the plan is a `consumer:alias=artifact`
//! line per edge, consumed by `jackdaw-rustc-wrapper`.
//!
//! Before planning, the extension's lockfile is aligned: closure crates
//! resolved at a semver-compatible but different version get pinned to
//! the SDK's exact version. The lockfile in question is the generated
//! shim crate's, never the user's.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sdk_paths::SdkPaths;

#[derive(Debug)]
pub enum PlanError {
    Io(std::io::Error),
    Cargo(String),
    Parse(String),
    /// The SDK itself is not usable, whatever the extension asked for.
    Sdk(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::Cargo(msg) => write!(f, "cargo failed: {msg}"),
            Self::Parse(msg) => write!(f, "could not parse: {msg}"),
            Self::Sdk(msg) => write!(f, "unusable SDK: {msg}"),
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
        // release SDK's manifest points at release rlibs (what extensions redirect
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
    /// (`-p jackdaw_sdk --release`). The SDK is already
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
        let closure = sdk_runtime_closure(workspace_root, &sdk.triple)?;

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

        // Cargo reports artifact paths with the platform's separator,
        // so matching only on `/` discarded every artifact on Windows
        // and produced an empty manifest. Nothing downstream can work
        // without it: the redirect plan has no edges, and an extension
        // silently compiles its own bevy instead of the SDK's.
        let in_triple_dir = |path: &str| {
            path.contains(&format!("/{}/", sdk.triple))
                || path.contains(&format!("\\{}\\", sdk.triple))
        };
        let roots = build_private_roots(sdk);
        let mut artifacts = BTreeMap::new();
        let mut link_paths: BTreeSet<String> = BTreeSet::new();
        let mut host_deps: BTreeSet<String> = BTreeSet::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            // Native search paths a build script emitted. A crate that
            // ships its own import libraries (the `windows` crates ship
            // `windows.0.52.0.lib` and friends) is only findable through
            // these, and they exist solely in the SDK's build. A consumer
            // inherits the `#[link]` directives through crate metadata but
            // not the directories, so its link fails on a bare library
            // name.
            //
            // Only directories private to this build are recorded.
            // pkg-config emits system directories such as
            // `/usr/lib/x86_64-linux-gnu`, and those already exist on the
            // consumer's machine and are already on its default search
            // path. Recording one is worse than useless: the bundler
            // copies every archive it holds, so a static `libc.a` lands
            // ahead of the sysroot's `libc.so` and an extension dylib fails
            // to link on relocations that only a static executable
            // allows.
            if msg["reason"] == "build-script-executed" {
                for path in msg["linked_paths"].as_array().into_iter().flatten() {
                    let Some(path) = path.as_str() else { continue };
                    // Entries are `kind=path` or a bare path.
                    let bare = path.split_once('=').map_or(path, |(_, rest)| rest);
                    if !bare.is_empty() && private_to_this_build(Path::new(bare), &roots) {
                        link_paths.insert(bare.to_string());
                    }
                }
                continue;
            }
            if msg["reason"] != "compiler-artifact" {
                continue;
            }
            // Keyed on the package name, not the lib target's. The two
            // usually agree, and where they do not (`coreaudio-rs` builds
            // a lib called `coreaudio`) a target-name key matches nothing
            // in the closure, so the crate silently drops out of the
            // manifest. `write_plan` looks these up by package name too,
            // and an extension whose edge finds no entry compiles its own
            // copy instead of the SDK's, which is a type-identity
            // divergence nothing reports.
            let Some(pkg_id) = msg["package_id"].as_str() else {
                continue;
            };
            let (Some(name), Some(version)) = (package_id_name(pkg_id), package_id_version(pkg_id))
            else {
                continue;
            };
            let kinds = msg["target"]["kind"]
                .as_array()
                .map(|k| k.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            // Proc-macro dylibs, recorded by exact path like the rlibs.
            // Staging used to scoop every dylib in the host deps dir,
            // and that dir accumulates generations from other package
            // selections: an SDK whose rlibs referenced one generation
            // shipped beside proc macros from another, and an extension
            // build failed with `metadata mismatch` on every candidate,
            // reported as `can't find crate` for whichever SDK crate
            // held the chain. Kept before the closure filter because the
            // closure is computed with `no-proc-macro`.
            if kinds.contains(&"proc-macro") {
                for file in msg["filenames"].as_array().into_iter().flatten() {
                    if let Some(file) = file.as_str()
                        && file.ends_with(std::env::consts::DLL_SUFFIX)
                    {
                        host_deps.insert(file.to_string());
                    }
                }
                continue;
            }
            if !closure.contains(&(name.clone(), version.to_string())) {
                continue;
            }
            if kinds.contains(&"custom-build") {
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
                .filter(|f| in_triple_dir(f))
                .find(|f| f.ends_with(".rlib"))
                .or_else(|| {
                    filenames
                        .iter()
                        .filter_map(|f| f.as_str())
                        .filter(|f| in_triple_dir(f))
                        .find(|f| f.ends_with(".rmeta"))
                });
            if let Some(artifact) = artifact {
                artifacts.insert((name, version.to_string()), artifact.to_string());
            }
        }

        // A crate the closure names and this build produced no artifact
        // for is not fatal on its own: `cargo tree -p jackdaw_sdk`
        // resolves features for a narrower package selection than the
        // build, so the two can legitimately differ. It is worth saying
        // out loud, because the manifest is what every extension redirects
        // against and a gap here becomes a missing artifact there.
        let uncovered: Vec<String> = closure
            .iter()
            .filter(|key| !artifacts.contains_key(*key))
            .map(|(name, version)| format!("{name} {version}"))
            .collect();
        // The SDK build runs from the bundler and from a CLI, neither of
        // which installs a tracing subscriber, so a `warn!` here would
        // go nowhere.
        #[expect(clippy::print_stderr, reason = "no subscriber is installed here")]
        if !uncovered.is_empty() {
            eprintln!(
                "jackdaw: the SDK build produced no {} artifact for {} of the {} crates in \
                 jackdaw_sdk's runtime closure: {}",
                sdk.triple,
                uncovered.len(),
                closure.len(),
                uncovered.join(", ")
            );
        }

        write_link_paths(&link_paths_path(&sdk.manifest), &link_paths)?;
        write_lines(&host_deps_path(&sdk.manifest), &host_deps)?;
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
    /// Every `(name, version)` key in the manifest.
    pub fn names(&self) -> impl Iterator<Item = &(String, String)> {
        self.artifacts.keys()
    }

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
///
/// The result is absolute even when `deps_dir` is not. rustc runs from
/// the generated shim's directory, not from wherever the SDK was
/// resolved, so a relative `--extern` path points at nothing there and
/// the compile fails with `can't find crate` naming whichever crate
/// held the edge. `cargo xtask bundle --workspace .` is enough to
/// produce a relative `deps_dir`.
fn resolve_artifact(artifact: &str, deps_dir: &Path) -> String {
    let path = Path::new(artifact);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        deps_dir.join(artifact)
    };
    absolute(&joined).to_string_lossy().into_owned()
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
    let mut absent: BTreeSet<String> = BTreeSet::new();
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
                if !Path::new(&resolved).exists() {
                    absent.insert(format!("{dep_name} {dep_version} ({resolved})"));
                    continue;
                }
                writeln!(contents, "{consumer}@{consumer_version}:{alias}={resolved}")?;
                edges += 1;
            }
        }
    }
    // An SDK whose manifest names an artifact it does not hold is
    // broken, and every project built against it fails. Left to the
    // compile, rustc reports it as `can't find crate` against a `use` in
    // whichever third-party crate happened to hold the edge, pointing
    // nowhere near the SDK. Said here it costs seconds instead of a
    // whole build, and names the crate.
    if !absent.is_empty() {
        return Err(PlanError::Sdk(format!(
            "its manifest names {} artifact(s) that are not on disk, so nothing can be built \
             against it. Missing: {}",
            absent.len(),
            absent.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    // Every package the manifest holds an artifact for, so the wrapper
    // can recognise a plan-replaced crate's own compile by PACKAGE name.
    // Deriving that set from the edge aliases is wrong for renamed
    // dependencies: rustix consumes `errno` as `libc_errno`, so an
    // alias-keyed set sent rustix's shadow unit vanilla while errno's
    // stayed rewritten, and the two disagreed about which `libc` the
    // universe contains. rustc reported it as a bare "can't find crate
    // for errno" on both platforms.
    for (name, _version) in manifest.names() {
        writeln!(contents, "replaced:{name}")?;
    }
    if std::fs::read(out_path).ok().as_deref() != Some(contents.as_slice()) {
        std::fs::write(out_path, contents)?;
    }
    Ok(edges)
}

/// The SDK dylib's runtime dependency closure: `(name, version)`
/// pairs from `cargo tree`. Dev-checkout only.
///
/// Resolved for `triple`, not the host: platform-conditional
/// dependencies differ (52 crates separate a Linux graph from a macOS
/// one), and the closure is compared against artifacts from a
/// `--target triple` build. Taking the host's answer while
/// cross-compiling would name crates the target build never has.
fn sdk_runtime_closure(
    workspace_root: &Path,
    triple: &str,
) -> Result<BTreeSet<(String, String)>, PlanError> {
    let output = Command::new("cargo")
        .args([
            "tree",
            "-p",
            "jackdaw_sdk",
            "-e",
            "normal,no-proc-macro",
            "--prefix",
            "none",
            "--target",
            triple,
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
        let root = std::env::temp_dir().join("jackdaw_resolve_artifact");
        let deps = root.join("sdk/x86_64/deps");
        let built = root.join("target/x86_64/release/deps/libglam-abc.rlib");
        // Dev/bootstrap manifests store absolute paths: used verbatim.
        assert_eq!(
            PathBuf::from(resolve_artifact(&built.to_string_lossy(), &deps)),
            built
        );
        // A shipped manifest stores basenames, rebased onto the install deps.
        assert_eq!(
            PathBuf::from(resolve_artifact("libglam-abc.rlib", &deps)),
            deps.join("libglam-abc.rlib")
        );
    }

    /// rustc runs from the shim's directory, so a relative result would
    /// name a file that is not there and fail the compile against a
    /// crate the user never mentioned.
    #[test]
    fn a_relative_deps_dir_still_resolves_to_an_absolute_artifact() {
        let resolved = resolve_artifact("libglam-abc.rlib", Path::new("./target/sdk/deps"));
        assert!(
            Path::new(&resolved).is_absolute(),
            "{resolved} has to be absolute"
        );
        assert!(resolved.ends_with("libglam-abc.rlib"), "{resolved}");
    }
}

/// The directory trees whose contents exist only because this build ran:
/// the target directory (where build scripts compile native code into
/// their `OUT_DIR`) and the cargo home (where crates such as the
/// `windows` family ship prebuilt import libraries). A link search path
/// under either travels with the SDK; anything else belongs to the
/// machine and is already on the consumer's default search path.
/// Roots are absolutised because the paths they are matched against
/// are: cargo reports an absolute `linked_paths`, while the SDK's own
/// dirs are only as absolute as the caller made them. `cargo xtask
/// bundle --workspace .` gives a relative root, and a relative root
/// matches no absolute path at all, so the filter would silently drop
/// every entry and the SDK would ship without the search paths a
/// consumer's link needs.
fn build_private_roots(sdk: &SdkPaths) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    // `deps` is `<target>/<triple>/<profile>/deps`; its parent holds the
    // sibling `build/<pkg>-<hash>/out` directories.
    if let Some(profile_dir) = sdk.deps.parent() {
        roots.push(absolute(profile_dir));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".cargo"))
        })
        .map(|home| absolute(&home));
    roots.extend(cargo_home);
    roots
}

/// `path` resolved against the current directory, with symlinks
/// followed where it exists. Falls back to the path as given rather
/// than failing: a root that cannot be resolved still matches whatever
/// it literally prefixes.
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn private_to_this_build(path: &Path, roots: &[PathBuf]) -> bool {
    let resolved = absolute(path);
    roots
        .iter()
        .any(|root| resolved.starts_with(root) || path.starts_with(root))
}

/// Where the proc-macro dylib list sits, beside the manifest it was
/// captured with.
pub fn host_deps_path(manifest: &Path) -> PathBuf {
    manifest.with_file_name("jackdaw_sdk_host_deps.txt")
}

/// The exact proc-macro dylibs the SDK build produced, if recorded.
/// Absent means an SDK from before this was captured; the caller falls
/// back to staging the whole host deps directory.
pub fn read_host_deps(manifest: &Path) -> Vec<String> {
    std::fs::read_to_string(host_deps_path(manifest))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn write_lines(path: &Path, lines: &BTreeSet<String>) -> Result<(), PlanError> {
    let contents: String = lines.iter().map(|l| format!("{l}\n")).collect();
    if std::fs::read_to_string(path).ok().as_deref() != Some(contents.as_str()) {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Where the native link search paths sit, beside the manifest they were
/// captured with.
pub fn link_paths_path(manifest: &Path) -> PathBuf {
    manifest.with_file_name("jackdaw_sdk_link_paths.txt")
}

/// Record the directories a consumer's linker needs on its search path,
/// one per line. Written even when empty, so a stale file from an
/// earlier SDK never outlives the build that produced it.
fn write_link_paths(path: &Path, paths: &BTreeSet<String>) -> Result<(), PlanError> {
    let contents: String = paths.iter().map(|p| format!("{p}\n")).collect();
    if std::fs::read_to_string(path).ok().as_deref() != Some(contents.as_str()) {
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// The native search paths recorded for this SDK, if any. Absent or
/// unreadable means none, which is the correct answer for an SDK built
/// before this was captured.
pub fn read_link_paths(manifest: &Path) -> Vec<String> {
    std::fs::read_to_string(link_paths_path(manifest))
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod link_path_tests {
    use super::*;

    /// Cargo reports these as `kind=path`, and a consumer needs the bare
    /// directory. An SDK built before this was captured has no file, and
    /// must read as "none" rather than failing.
    #[test]
    fn link_paths_round_trip_and_tolerate_absence() {
        let dir = std::env::temp_dir().join(format!("jackdaw_linkpaths_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("jackdaw_sdk_manifest.txt");

        assert!(
            read_link_paths(&manifest).is_empty(),
            "an SDK with no recorded paths reports none"
        );

        let mut paths = BTreeSet::new();
        paths.insert("/sdk/windows_x86_64_msvc/lib".to_string());
        paths.insert("/sdk/other/lib".to_string());
        write_link_paths(&link_paths_path(&manifest), &paths).unwrap();

        let read = read_link_paths(&manifest);
        assert_eq!(read.len(), 2, "{read:?}");
        assert!(read.contains(&"/sdk/windows_x86_64_msvc/lib".to_string()));

        // Rewriting with none clears it, so a stale list cannot outlive
        // the build that produced it.
        write_link_paths(&link_paths_path(&manifest), &BTreeSet::new()).unwrap();
        assert!(read_link_paths(&manifest).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A build script that ran pkg-config emits system directories, and
    /// the bundler copies every archive in a directory it is given. Left
    /// unfiltered, `/usr/lib/x86_64-linux-gnu/libc.a` ships in the SDK
    /// and shadows the sysroot's shared libc, which fails a project's
    /// dylib link on `R_X86_64_TPOFF32` relocations.
    #[test]
    fn only_directories_this_build_created_are_recorded() {
        let roots = vec![
            PathBuf::from("/work/target/x86_64-unknown-linux-gnu/release"),
            PathBuf::from("/home/user/.cargo"),
        ];
        for private in [
            "/work/target/x86_64-unknown-linux-gnu/release/build/blake3-abc/out",
            "/home/user/.cargo/registry/src/index.crates.io-1/windows_x86_64_msvc-0.52.6/lib",
        ] {
            assert!(
                private_to_this_build(Path::new(private), &roots),
                "{private} exists only where this build ran"
            );
        }
        for system in [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/local/lib",
            "/opt/homebrew/lib",
        ] {
            assert!(
                !private_to_this_build(Path::new(system), &roots),
                "{system} belongs to the machine, not the SDK"
            );
        }
    }

    /// `cargo xtask bundle --workspace .` resolves the SDK under a
    /// relative root while cargo reports `linked_paths` absolute. Left
    /// literal, the two never match and every path is dropped, which
    /// costs a consumer the search paths its link needs and shows up
    /// only as a bare library name the linker cannot find.
    #[test]
    fn a_relative_root_still_matches_the_absolute_path_cargo_reports() {
        let cwd = std::env::current_dir().expect("a current dir");
        let roots =
            build_private_roots(&SdkPaths::for_workspace_profile(Path::new("."), "release"));
        let out_dir = cwd
            .join("target")
            .join(crate::sdk_paths::host_triple())
            .join("release/build/blake3-abc/out");
        assert!(
            private_to_this_build(&out_dir, &roots),
            "{} is under the relative root {:?}",
            out_dir.display(),
            roots
        );
        assert!(
            !private_to_this_build(Path::new("/usr/lib"), &roots),
            "a system dir is still not this build's"
        );
    }

    /// The file sits beside the manifest so both travel together, in a
    /// bundle or a cache.
    #[test]
    fn the_link_path_file_sits_beside_the_manifest() {
        let manifest = Path::new("/sdk/x86_64/jackdaw_sdk_manifest.txt");
        assert_eq!(
            link_paths_path(manifest),
            Path::new("/sdk/x86_64/jackdaw_sdk_link_paths.txt")
        );
    }
}

#[cfg(test)]
mod triple_dir_tests {
    /// Cargo emits artifact paths with the platform's own separator.
    /// Matching only forward slashes discarded every artifact on
    /// Windows, so the manifest came out empty and the redirect plan had
    /// nothing to redirect.
    #[test]
    fn the_triple_dir_matches_either_separator() {
        let triple = "x86_64-pc-windows-msvc";
        let matches = |path: &str| {
            path.contains(&format!("/{triple}/")) || path.contains(&format!("\\{triple}\\"))
        };

        assert!(matches(
            "D:\\a\\jackdaw\\target\\x86_64-pc-windows-msvc\\debug\\deps\\libbevy.rlib"
        ));
        assert!(matches(
            "/home/joe/jackdaw/target/x86_64-pc-windows-msvc/debug/deps/libbevy.rlib"
        ));
        // A host-side unit is not in the triple dir and must stay out.
        assert!(!matches(
            "D:\\a\\jackdaw\\target\\debug\\deps\\libbevy.rlib"
        ));
        assert!(!matches("/home/joe/jackdaw/target/debug/deps/libbevy.rlib"));
    }
}
