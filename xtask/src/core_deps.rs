//! Keeping the core crates on Bevy alone.
//!
//! Every crate in the main workspace may depend on Bevy's own crates and on
//! crates that do not depend on Bevy, never on a third-party crate built on
//! Bevy. Integrations with such crates live in the `integrations/` workspaces.

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Stdio};

/// The repository every crate of Bevy itself names.
const BEVY_REPOSITORY: &str = "https://github.com/bevyengine/bevy";

/// Check the workspace with every feature on, so an optional dependency
/// counts as much as a default one.
pub fn cmd() -> bool {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ])
        .stderr(Stdio::inherit())
        .output();
    let metadata: serde_json::Value = match output {
        Ok(output) if output.status.success() => match serde_json::from_slice(&output.stdout) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!("parse cargo metadata: {error}");
                return false;
            }
        },
        Ok(_) => {
            eprintln!("cargo metadata failed");
            return false;
        }
        Err(error) => {
            eprintln!("run cargo metadata: {error}");
            return false;
        }
    };
    let violations = violations(&metadata);
    for (dependency, violation) in &violations {
        eprintln!(
            "{dependency} depends on Bevy through {}, and core crates use it: {}",
            violation.path.join(" -> "),
            violation
                .users
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if violations.is_empty() {
        eprintln!("the core crates depend on no third-party Bevy crate");
    }
    violations.is_empty()
}

/// A third-party crate that brings Bevy into the core.
#[derive(Debug, Default, PartialEq)]
pub struct Violation {
    /// The workspace crates that depend on it directly.
    pub users: BTreeSet<String>,
    /// Package names from it down to a Bevy crate.
    pub path: Vec<String>,
}

/// Every direct, non-dev dependency of a workspace crate that is neither in
/// the workspace nor part of Bevy, yet reaches a Bevy crate.
pub fn violations(metadata: &serde_json::Value) -> BTreeMap<String, Violation> {
    let packages: BTreeMap<&str, &serde_json::Value> = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect();
    let name = |id: &str| {
        packages
            .get(id)
            .and_then(|package| package["name"].as_str())
            .unwrap_or(id)
            .to_string()
    };
    let is_bevy = |id: &str| {
        packages.get(id).is_some_and(|package| {
            package["repository"]
                .as_str()
                .is_some_and(|repository| repository.trim_end_matches('/') == BEVY_REPOSITORY)
        })
    };
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let edges: BTreeMap<&str, Vec<&str>> = metadata["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let deps = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dep| {
                    dep["dep_kinds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|kind| kind["kind"].as_str() != Some("dev"))
                })
                .filter_map(|dep| dep["pkg"].as_str())
                .collect();
            Some((node["id"].as_str()?, deps))
        })
        .collect();

    let mut to_bevy: BTreeMap<&str, Option<Vec<&str>>> = BTreeMap::new();
    let mut violations: BTreeMap<String, Violation> = BTreeMap::new();
    for member in &members {
        for dep in edges.get(member).into_iter().flatten() {
            if members.contains(dep) || is_bevy(dep) {
                continue;
            }
            let Some(path) = path_to_bevy(dep, &edges, &is_bevy, &mut to_bevy) else {
                continue;
            };
            let violation = violations.entry(name(dep)).or_default();
            violation.users.insert(name(member));
            violation.path = path.iter().map(|id| name(id)).collect();
        }
    }
    violations
}

/// The shortest-known chain of packages from `id` to a Bevy crate.
fn path_to_bevy<'a>(
    id: &'a str,
    edges: &BTreeMap<&'a str, Vec<&'a str>>,
    is_bevy: &dyn Fn(&str) -> bool,
    memo: &mut BTreeMap<&'a str, Option<Vec<&'a str>>>,
) -> Option<Vec<&'a str>> {
    if is_bevy(id) {
        return Some(vec![id]);
    }
    if let Some(known) = memo.get(id) {
        return known.clone();
    }
    memo.insert(id, None);
    let mut best: Option<Vec<&str>> = None;
    for dep in edges.get(id).into_iter().flatten() {
        if let Some(rest) = path_to_bevy(dep, edges, is_bevy, memo)
            && best.as_ref().is_none_or(|best| rest.len() + 1 < best.len())
        {
            best = Some(std::iter::once(id).chain(rest).collect());
        }
    }
    memo.insert(id, best.clone());
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(id: &str, repository: Option<&str>) -> serde_json::Value {
        serde_json::json!({ "id": id, "name": id, "repository": repository })
    }

    fn node(id: &str, deps: &[(&str, Option<&str>)]) -> serde_json::Value {
        let deps: Vec<serde_json::Value> = deps
            .iter()
            .map(|(pkg, kind)| serde_json::json!({ "pkg": pkg, "dep_kinds": [{ "kind": kind }] }))
            .collect();
        serde_json::json!({ "id": id, "deps": deps })
    }

    #[test]
    fn only_third_party_crates_that_reach_bevy_are_reported() {
        let bevy = Some(BEVY_REPOSITORY);
        let metadata = serde_json::json!({
            "workspace_members": ["editor", "runtime"],
            "packages": [
                package("editor", None),
                package("runtime", None),
                package("bevy", bevy),
                package("bevy_ecs", bevy),
                package("glam", None),
                package("physics", None),
                package("physics_core", None),
                package("test_helper", None),
            ],
            "resolve": { "nodes": [
                node("editor", &[
                    ("runtime", None),
                    ("bevy", None),
                    ("glam", None),
                    ("physics", None),
                    ("test_helper", Some("dev")),
                ]),
                node("runtime", &[("bevy_ecs", None), ("physics", Some("build"))]),
                node("bevy", &[("bevy_ecs", None)]),
                node("bevy_ecs", &[("glam", None)]),
                node("glam", &[]),
                node("physics", &[("physics_core", None)]),
                node("physics_core", &[("bevy_ecs", None)]),
                node("test_helper", &[("bevy", None)]),
            ]}
        });
        let violations = violations(&metadata);
        assert_eq!(
            violations,
            BTreeMap::from([(
                "physics".to_string(),
                Violation {
                    users: BTreeSet::from(["editor".to_string(), "runtime".to_string()]),
                    path: vec![
                        "physics".to_string(),
                        "physics_core".to_string(),
                        "bevy_ecs".to_string()
                    ],
                }
            )])
        );
    }
}
