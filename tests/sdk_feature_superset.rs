#![expect(clippy::print_stdout, reason = "test reports what it compared")]
//! A project must never resolve a feature the SDK it links did not.
//!
//! Cargo resolves features per package selection. The SDK is built from
//! `jackdaw_sdk` (plus the runner); a project is built from the shim
//! rooted at the user's crate. When the project's selection turns on a
//! feature the SDK's did not, the project compiles code expecting an
//! impl that the SDK rlib it links was built without, and rustc reports
//! it against a crate the user never named. That shipped once:
//! `-p jackdaw_sdk` resolved `glam` without `approx` while a scaffolded
//! project pulled `bevy_math/approx` through the physics integration, so
//! every project built against a prepared SDK failed inside `bevy_math`.
//!
//! `sdk_feature_closure.rs` guards the manifest that fixed that case.
//! This checks the property itself, against real resolutions, so a
//! divergence arriving by any other route is caught too. It compares
//! `cargo tree -e features` output and compiles nothing.
//!
//! Every SDK-consuming test elsewhere uses the dev checkout's SDK, whose
//! selection is the whole editor and therefore a superset of everything.
//! That is why this hole stayed open: the narrow selection is only used
//! by the SDK a source-free install prepares for itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use jackdaw::project_build::shim;
use jackdaw::project_build::shim_spec_for_project;
use jackdaw::scaffold::{TemplateKind, scaffold_new_project};
use jackdaw::sdk_paths::host_triple;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `name v<version> -> features` as cargo resolves them for one package
/// selection.
///
/// Keyed by version because the redirect plan is: an edge is written
/// only where the project's resolved version of a dependency matches the
/// SDK's, so only those pairs can mismatch. Two majors of one crate
/// compare separately, which is what stops a graph that carries both
/// from reading as a divergence.
fn resolved_features(
    dir: &Path,
    packages: &[&str],
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut command = Command::new("cargo");
    command.arg("tree").args(["-e", "normal"]);
    for package in packages {
        command.args(["-p", package]);
    }
    command
        .args(["--target", host_triple(), "--prefix", "none"])
        .args(["--format", "{p}|{f}"])
        // Without this cargo colours the `(*)` already-shown marker on
        // some terminals, and the escape sequences end up inside the
        // parsed feature name. That read as a feature the SDK lacked
        // and failed the run for no reason.
        .args(["--color", "never"])
        .current_dir(dir);

    let output = command
        .output()
        .map_err(|e| format!("run cargo tree in {}: {e}", dir.display()))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut features: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // `name v1.2.3|feat_a,feat_b`, sometimes with a trailing ` (*)`
        // where cargo has already shown that package's subtree, and
        // sometimes with a path or source in the package field.
        let Some((package, feature_list)) = line.trim().split_once('|') else {
            continue;
        };
        let key = package
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let entry = features.entry(key).or_default();
        for feature in feature_list.split(',') {
            // Drop the already-shown marker wherever it lands, rather
            // than assuming it is a plain suffix.
            let feature = feature.split_whitespace().next().unwrap_or_default();
            if !feature.is_empty() && feature != "(*)" {
                entry.insert(feature.to_string());
            }
        }
    }
    Ok(features)
}

/// Scaffold a game and generate its shim, which is the crate a project
/// build is actually rooted at.
fn scaffold_with_shim(root: &Path) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(root);
    scaffold_new_project(root, "featureprobe", TemplateKind::Game)
        .map_err(|e| format!("scaffold {}: {e}", root.display()))?;

    let spec = shim_spec_for_project(root, None)
        .ok_or_else(|| format!("{} has no lib crate to build", root.display()))?;
    shim::ensure_shim(&spec, &root.join(".jackdaw")).map_err(|e| format!("generate shim: {e}"))
}

/// Skip when the environment cannot resolve dependencies (no registry
/// cache, no network). `JACKDAW_FEATURE_SUPERSET_REQUIRED` turns the
/// skip into a failure, which is how CI runs it.
fn skip_or_fail(reason: &str) {
    assert!(
        std::env::var_os("JACKDAW_FEATURE_SUPERSET_REQUIRED").is_none(),
        "sdk_feature_superset was required but {reason}"
    );
    println!("SKIP sdk_feature_superset: {reason}");
}

#[test]
fn a_project_resolves_no_feature_the_sdk_lacks() {
    let workspace = workspace_root();
    let sdk = match resolved_features(&workspace, &["jackdaw_sdk", "jackdaw_runner"]) {
        Ok(features) => features,
        Err(reason) => return skip_or_fail(&reason),
    };

    let project =
        std::env::temp_dir().join(format!("jackdaw_feature_probe_{}", std::process::id()));
    let shim = match scaffold_with_shim(&project) {
        Ok(shim) => shim,
        Err(reason) => {
            let _ = std::fs::remove_dir_all(&project);
            return skip_or_fail(&reason);
        }
    };
    let resolved = resolved_features(&shim, &[]);
    let _ = std::fs::remove_dir_all(&project);
    let resolved = match resolved {
        Ok(features) => features,
        Err(reason) => return skip_or_fail(&reason),
    };

    // Only packages both selections resolve to the same version can
    // diverge: anything else gets no redirect edge, so the project
    // compiles and links its own build with no mismatch to have.
    let mut divergent: Vec<String> = Vec::new();
    let mut shared = 0;
    for (package, wanted) in &resolved {
        let Some(available) = sdk.get(package) else {
            continue;
        };
        shared += 1;
        let missing: Vec<&String> = wanted.difference(available).collect();
        if !missing.is_empty() {
            divergent.push(format!("{package}: {missing:?}"));
        }
    }

    assert!(shared > 50, "expected a large shared closure, got {shared}");
    println!("compared {shared} crates shared by the SDK and a scaffolded project");
    assert!(
        divergent.is_empty(),
        "a scaffolded project resolves features the SDK does not, so it would compile against \
         SDK rlibs built without them:\n  {}\n\nEnable them from crates/jackdaw_sdk/Cargo.toml \
         so the SDK's resolution stays a superset of any project's.",
        divergent.join("\n  ")
    );
}
