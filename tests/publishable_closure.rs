//! The crates a scaffolded project depends on must stay publishable to
//! crates.io, or `jd new` produces a project that cannot resolve.
//!
//! This is not hypothetical: the editor depends on `bevy_rerecast` by
//! git, crates.io rejects git dependencies, and the release workflow
//! used to gate the whole workspace publish on that one crate. The
//! result was templates requesting versions that were never published.
//! The editor is excluded from the publish for that reason; these tests
//! pin the property that lets the rest go out without it.

use std::collections::{BTreeSet, HashMap};

/// The two crates the templates name. Everything they pull in has to be
/// published alongside them.
const SCAFFOLD_ROOTS: [&str; 2] = ["jackdaw_runtime", "jackdaw_extension"];

struct Package {
    deps: Vec<Dep>,
}

struct Dep {
    name: String,
    req: String,
    source: Option<String>,
    kind: Option<String>,
}

fn workspace() -> HashMap<String, Package> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata runs");
    assert!(output.status.success(), "cargo metadata failed");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata parses");
    json["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|package| {
            let name = package["name"].as_str().expect("name").to_string();
            let deps = package["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .map(|dep| Dep {
                    name: dep["name"].as_str().unwrap_or_default().to_string(),
                    req: dep["req"].as_str().unwrap_or_default().to_string(),
                    source: dep["source"].as_str().map(str::to_string),
                    kind: dep["kind"].as_str().map(str::to_string),
                })
                .collect();
            (name, Package { deps })
        })
        .collect()
}

/// Every workspace crate reachable from the scaffolding roots through
/// normal (non dev/build) dependencies.
fn scaffold_closure(packages: &HashMap<String, Package>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = SCAFFOLD_ROOTS
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    while let Some(name) = stack.pop() {
        if !packages.contains_key(&name) || !seen.insert(name.clone()) {
            continue;
        }
        for dep in &packages[&name].deps {
            if dep.kind.is_none() && packages.contains_key(&dep.name) {
                stack.push(dep.name.clone());
            }
        }
    }
    seen
}

#[test]
fn the_scaffold_closure_has_no_git_dependencies() {
    let packages = workspace();
    let offenders: Vec<String> = scaffold_closure(&packages)
        .iter()
        .flat_map(|name| {
            packages[name]
                .deps
                .iter()
                .filter(|dep| {
                    dep.kind.is_none()
                        && dep.source.as_deref().is_some_and(|s| s.starts_with("git+"))
                })
                .map(move |dep| format!("{name} -> {}", dep.name))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "crates.io rejects git dependencies, so these would block publishing the crates \
         `jd new` projects depend on:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn every_normal_intra_workspace_dependency_states_a_version() {
    let packages = workspace();
    let offenders: Vec<String> = scaffold_closure(&packages)
        .iter()
        .flat_map(|name| {
            packages[name]
                .deps
                .iter()
                // Only normal dependencies: cargo strips a version-less
                // dev-dependency from the published manifest, and the
                // geometry/scene-types cycle relies on exactly that.
                .filter(|dep| {
                    dep.kind.is_none() && packages.contains_key(&dep.name) && dep.req == "*"
                })
                .map(move |dep| format!("{name} -> {}", dep.name))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "`cargo publish` rejects a normal path dependency with no version requirement; add one \
         to:\n  {}",
        offenders.join("\n  ")
    );
}

/// `jackdaw_geometry` dev-depends on `jackdaw_scene_types`, which
/// depends back on it. Cargo only tolerates that cycle because the
/// dev-dependency states no version and is therefore dropped from the
/// published manifest. Adding one (an easy "fix" for a lint that only
/// applies to normal dependencies) makes both crates unpublishable, and
/// with them everything `jd new` produces.
#[test]
fn the_dev_dependency_cycle_stays_version_less() {
    let packages = workspace();
    let dep = packages["jackdaw_geometry"]
        .deps
        .iter()
        .find(|dep| dep.name == "jackdaw_scene_types")
        .expect("geometry dev-depends on scene_types");
    assert_eq!(dep.kind.as_deref(), Some("dev"));
    assert_eq!(
        dep.req, "*",
        "a version here re-forms the publish cycle; leave it version-less"
    );
}

/// `cargo publish -p X` publishes X alone, not X's path dependencies,
/// so the release workflow has to name every crate in the closure. A
/// dependency added later would otherwise be left off the registry and
/// every scaffolded project would fail to resolve against it.
#[test]
fn the_release_workflow_publishes_the_whole_closure() {
    let workflow = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yaml"
    ))
    .expect("release workflow");
    let listed: BTreeSet<String> = workflow
        .lines()
        .filter_map(|line| line.trim().strip_prefix("-p "))
        .map(|name| name.trim().to_string())
        .collect();
    let expected = scaffold_closure(&workspace());
    assert_eq!(
        listed, expected,
        "\nrelease.yaml publishes: {listed:#?}\nthe real closure is:    {expected:#?}"
    );
}

/// The editor is the crate that cannot be published, and the reason the
/// publish is scoped rather than workspace-wide. If it ever loses its
/// git dependency this scoping can go away.
#[test]
fn the_editor_is_what_keeps_the_publish_scoped() {
    let packages = workspace();
    let editor_has_git = packages["jackdaw"]
        .deps
        .iter()
        .any(|dep| dep.source.as_deref().is_some_and(|s| s.starts_with("git+")));
    assert!(
        editor_has_git,
        "the editor no longer has a git dependency, so `cargo publish --workspace` may now \
         work; revisit the scoped publish in .github/workflows/release.yaml"
    );
    assert!(
        !scaffold_closure(&packages).contains("jackdaw"),
        "the scaffolding closure must not reach the editor crate"
    );
}
