//! `cargo metadata` parsing: which package in a project (or workspace)
//! the editor should build, read without any editor or bevy dependency
//! so the build pipeline and the CLI can resolve a project from the
//! filesystem alone.

use std::path::{Path, PathBuf};

use jackdaw_env::rust_env_command;
use serde::Deserialize;

/// A project's cargo metadata: packages and their targets.
#[derive(Deserialize)]
pub struct CargoMeta {
    packages: Vec<MetaPackage>,
    #[serde(default)]
    workspace_root: String,
    #[serde(default)]
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct MetaPackage {
    name: String,
    /// Package id, matched against `workspace_members`. Optional so a
    /// hand-written metadata document (tests, fixtures) parses without
    /// it; `--no-deps` already limits packages to workspace members, so
    /// the filter is a second line of defence rather than the mechanism.
    #[serde(default)]
    id: String,
    #[serde(default)]
    manifest_path: String,
    targets: Vec<MetaTarget>,
    #[serde(default)]
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

/// One candidate package, flattened out of the metadata into what the
/// import and build flows actually branch on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    /// Package name as spelled in the manifest, dashes preserved.
    pub name: String,
    /// The lib target's crate name (underscored). Falls back to the
    /// underscored package name when the package has no lib target.
    pub crate_name: String,
    /// Directory holding this package's `Cargo.toml`.
    pub dir: PathBuf,
    pub has_lib: bool,
    pub has_bin: bool,
    /// The package depends directly on `bevy` or a `bevy_*` crate.
    /// A member can be the game without this: a split workspace often
    /// gets bevy transitively through a sibling crate.
    pub depends_on_bevy: bool,
}

impl PackageInfo {
    /// Whether this package's sources contain a Bevy `Plugin` impl. The
    /// second signal for "this is the game", read from disk rather than
    /// from the dependency graph.
    pub fn declares_a_plugin(&self) -> bool {
        !crate::detect::plugin_candidates(&self.dir).is_empty()
    }
}

/// Why a project's target package could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// `cargo metadata` could not run or did not parse (no manifest, a
    /// manifest error, or no cargo on PATH). `detail` carries cargo's
    /// own message when there was one, since a manifest error is
    /// usually the real problem and only cargo can explain it.
    NoMetadata { root: PathBuf, detail: String },
    /// A workspace with no package that looks like a Bevy game.
    NoBevyPackage { candidates: Vec<String> },
    /// Several packages qualify and none was requested.
    Ambiguous { candidates: Vec<String> },
    /// An explicitly requested package is not in this workspace.
    UnknownPackage {
        requested: String,
        candidates: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoMetadata { root, detail } if detail.is_empty() => write!(
                f,
                "could not read cargo metadata for {} (is there a valid Cargo.toml, and is cargo \
                 on PATH?)",
                root.display()
            ),
            Self::NoMetadata { root, detail } => {
                write!(f, "cargo could not read {}: {detail}", root.display())
            }
            Self::NoBevyPackage { candidates } if candidates.is_empty() => {
                write!(f, "there are no packages here to import")
            }
            // A single-package project is not "a workspace", and saying
            // so sends the user looking for members that do not exist.
            Self::NoBevyPackage { candidates } if candidates.len() == 1 => write!(
                f,
                "`{}` does not depend on bevy, so there is no game to set up here",
                candidates[0]
            ),
            Self::NoBevyPackage { candidates } => write!(
                f,
                "no package in this workspace depends on bevy; pick one explicitly with \
                 --package (found: {})",
                candidates.join(", ")
            ),
            Self::Ambiguous { candidates } => write!(
                f,
                "several packages here could be the game; pick one with --package (found: {})",
                candidates.join(", ")
            ),
            Self::UnknownPackage {
                requested,
                candidates,
            } => write!(
                f,
                "`{requested}` is not a package here (found: {})",
                candidates.join(", ")
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

impl CargoMeta {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Run `cargo metadata --no-deps` in `project_dir`.
    pub fn load(project_dir: &Path) -> Option<Self> {
        Self::load_or_error(project_dir).ok()
    }

    /// As [`CargoMeta::load`], keeping cargo's diagnostics so an import
    /// can tell the user why their manifest was rejected.
    pub fn load_or_error(project_dir: &Path) -> Result<Self, ResolveError> {
        let fail = |detail: String| ResolveError::NoMetadata {
            root: project_dir.to_path_buf(),
            detail,
        };
        let out = rust_env_command("cargo")
            .current_dir(project_dir)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .output()
            .map_err(|e| fail(format!("could not run cargo: {e}")))?;
        if !out.status.success() {
            return Err(fail(first_error_line(&String::from_utf8_lossy(
                &out.stderr,
            ))));
        }
        Self::parse(&String::from_utf8_lossy(out.stdout.as_slice()))
            .map_err(|e| fail(e.to_string()))
    }

    /// The workspace root directory, when the metadata reported one.
    pub fn workspace_root(&self) -> Option<PathBuf> {
        (!self.workspace_root.is_empty()).then(|| PathBuf::from(&self.workspace_root))
    }

    /// Every workspace member, in manifest order.
    pub fn packages(&self) -> Vec<PackageInfo> {
        self.packages
            .iter()
            .filter(|p| {
                self.workspace_members.is_empty()
                    || p.id.is_empty()
                    || self.workspace_members.contains(&p.id)
            })
            .map(|p| {
                let lib = p
                    .targets
                    .iter()
                    .find(|t| t.kind.iter().any(|k| k == "lib" || k == "rlib"));
                PackageInfo {
                    name: p.name.clone(),
                    crate_name: lib
                        .map(|t| t.name.replace('-', "_"))
                        .unwrap_or_else(|| p.name.replace('-', "_")),
                    dir: Path::new(&p.manifest_path)
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default(),
                    has_lib: lib.is_some(),
                    has_bin: p.targets.iter().any(|t| t.kind.iter().any(|k| k == "bin")),
                    depends_on_bevy: p
                        .dependencies
                        .iter()
                        .any(|d| d.name == "bevy" || d.name.starts_with("bevy_")),
                }
            })
            .collect()
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
        self.packages()
            .into_iter()
            .find(|p| p.has_lib)
            .map(|p| (p.name, p.crate_name))
    }

    /// The package an import or build should target.
    ///
    /// A single-package project resolves to itself. In a workspace the
    /// Bevy-depending members are the candidates: exactly one resolves
    /// silently, several need `preferred` to disambiguate. `preferred`
    /// always wins when it names a real member, so a user can point at a
    /// package the heuristic would not have picked.
    pub fn resolve_package(&self, preferred: Option<&str>) -> Result<PackageInfo, ResolveError> {
        let packages = self.packages();
        let names = |list: &[PackageInfo]| list.iter().map(|p| p.name.clone()).collect::<Vec<_>>();

        if let Some(requested) = preferred {
            return packages
                .iter()
                .find(|p| p.name == requested || p.crate_name == requested.replace('-', "_"))
                .cloned()
                .ok_or_else(|| ResolveError::UnknownPackage {
                    requested: requested.to_string(),
                    candidates: names(&packages),
                });
        }

        // Rank by how strongly a member looks like the game. A direct
        // bevy dependency is the clearest signal, but a workspace that
        // splits crates often has the plugin in a member that only gets
        // bevy transitively, so a `impl Plugin for` in its sources
        // counts too. Nothing is short-circuited on package count: a
        // one-member workspace of a CLI tool is not a game, and used to
        // be accepted silently.
        // `declares_a_plugin` walks a package's sources, so evaluate it
        // once per package rather than again on the tie-break branch.
        let scanned: Vec<(PackageInfo, bool)> = packages
            .iter()
            .map(|p| (p.clone(), p.declares_a_plugin()))
            .collect();
        let game_like: Vec<(PackageInfo, bool)> = scanned
            .iter()
            .filter(|(p, has_plugin)| p.depends_on_bevy || *has_plugin)
            .cloned()
            .collect();
        match game_like.as_slice() {
            [(only, _)] => Ok(only.clone()),
            [] => Err(ResolveError::NoBevyPackage {
                candidates: names(&packages),
            }),
            // Hosting the plugin is the stronger signal: it is the type
            // the generated shim has to name. One member declaring a
            // plugin settles an otherwise ambiguous workspace, whether
            // the others merely name bevy or get it transitively.
            several => {
                let with_plugin: Vec<&PackageInfo> = several
                    .iter()
                    .filter(|(_, has_plugin)| *has_plugin)
                    .map(|(package, _)| package)
                    .collect();
                match with_plugin.as_slice() {
                    [only] => Ok((*only).clone()),
                    _ => Err(ResolveError::Ambiguous {
                        candidates: several
                            .iter()
                            .map(|(package, _)| package.name.clone())
                            .collect(),
                    }),
                }
            }
        }
    }
}

/// Resolve the package a project root refers to, running `cargo
/// metadata` for it. The one entry point the import planner, the CLI,
/// and the editor's shim builder share.
pub fn resolve_project_package(
    root: &Path,
    preferred: Option<&str>,
) -> Result<PackageInfo, ResolveError> {
    CargoMeta::load_or_error(root)?.resolve_package(preferred)
}

/// Condense a cargo error block into the line worth showing. Cargo
/// leads with `error: ...` and follows with location context; the first
/// error line plus the immediately following `  --> ` line is enough to
/// act on and short enough for a dialog.
fn first_error_line(stderr: &str) -> String {
    let mut lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .skip_while(|line| !line.starts_with("error"));
    let Some(first) = lines.next() else {
        return stderr.trim().lines().next().unwrap_or_default().to_string();
    };
    match lines.next().filter(|next| next.starts_with("-->")) {
        Some(location) => format!("{first} ({})", location.trim_start_matches("--> ")),
        None => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(json: &str) -> CargoMeta {
        CargoMeta::parse(json).expect("metadata parses")
    }

    /// A metadata document with the fields the resolver reads.
    fn workspace_json(packages: &[(&str, &str, &[&str], bool)]) -> String {
        let entries: Vec<serde_json::Value> = packages
            .iter()
            .map(|(name, dir, deps, has_lib)| {
                let deps: Vec<serde_json::Value> = deps
                    .iter()
                    .map(|dependency| serde_json::json!({ "name": dependency }))
                    .collect();
                let targets = if *has_lib {
                    serde_json::json!({ "name": name.replace('-', "_"), "kind": ["lib"] })
                } else {
                    serde_json::json!({ "name": name, "kind": ["bin"] })
                };
                serde_json::json!({
                    "name": name,
                    "id": format!("id-{name}"),
                    "manifest_path": Path::new(dir).join("Cargo.toml"),
                    "targets": [targets],
                    "dependencies": deps,
                })
            })
            .collect();
        let members: Vec<String> = packages
            .iter()
            .map(|(name, ..)| format!("id-{name}"))
            .collect();
        serde_json::json!({
            "packages": entries,
            "workspace_root": Path::new("/ws"),
            "workspace_members": members,
        })
        .to_string()
    }

    #[test]
    fn single_package_resolves_to_itself() {
        let m = meta(&workspace_json(&[("my-game", "/ws", &["bevy"], true)]));
        let pkg = m.resolve_package(None).unwrap();
        assert_eq!(pkg.name, "my-game");
        assert_eq!(pkg.crate_name, "my_game");
        assert_eq!(pkg.dir, PathBuf::from("/ws"));
        assert!(pkg.has_lib);
        assert!(pkg.depends_on_bevy);
    }

    #[test]
    fn workspace_picks_the_only_bevy_member() {
        let m = meta(&workspace_json(&[
            ("tools", "/ws/tools", &["clap"], true),
            ("game", "/ws/game", &["bevy"], true),
        ]));
        assert_eq!(m.resolve_package(None).unwrap().name, "game");
    }

    #[test]
    fn several_bevy_members_are_ambiguous_until_requested() {
        let m = meta(&workspace_json(&[
            ("client", "/ws/client", &["bevy"], true),
            ("server", "/ws/server", &["bevy_ecs"], true),
        ]));
        assert!(matches!(
            m.resolve_package(None),
            Err(ResolveError::Ambiguous { .. })
        ));
        assert_eq!(m.resolve_package(Some("server")).unwrap().name, "server");
        // The underscored crate name is accepted as well as the package name.
        assert_eq!(m.resolve_package(Some("client")).unwrap().name, "client");
    }

    #[test]
    fn a_workspace_without_bevy_reports_its_members() {
        let m = meta(&workspace_json(&[
            ("tools", "/ws/tools", &["clap"], true),
            ("data", "/ws/data", &["serde"], true),
        ]));
        let err = m.resolve_package(None).unwrap_err();
        let ResolveError::NoBevyPackage { candidates } = &err else {
            panic!("expected NoBevyPackage, got {err:?}");
        };
        assert_eq!(candidates, &["tools".to_string(), "data".to_string()]);
        assert!(err.to_string().contains("--package"));
    }

    #[test]
    fn an_unknown_request_lists_what_is_available() {
        let m = meta(&workspace_json(&[("game", "/ws", &["bevy"], true)]));
        let err = m.resolve_package(Some("nope")).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownPackage { .. }));
        assert!(err.to_string().contains("game"));
    }

    #[test]
    fn bin_only_packages_are_still_candidates() {
        let m = meta(&workspace_json(&[("game", "/ws", &["bevy"], false)]));
        let pkg = m.resolve_package(None).unwrap();
        assert!(!pkg.has_lib);
        assert!(pkg.has_bin);
        // Code still needs a crate name to reference; fall back to the
        // underscored package name so the import can plan a lib for it.
        assert_eq!(pkg.crate_name, "game");
    }

    /// Real directories, so the on-disk `impl Plugin for` signal is
    /// exercised rather than stubbed.
    fn scratch(name: &str, members: &[(&str, bool)]) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_resolve_{name}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (member, has_plugin) in members {
            let src = dir.join(member).join("src");
            std::fs::create_dir_all(&src).unwrap();
            let body = if *has_plugin {
                "impl Plugin for GamePlugin {}\n"
            } else {
                "pub fn helper() {}\n"
            };
            std::fs::write(src.join("lib.rs"), body).unwrap();
        }
        dir
    }

    fn json_at(root: &Path, packages: &[(&str, &str, &[&str], bool)]) -> String {
        let rebased: Vec<(String, String, &[&str], bool)> = packages
            .iter()
            .map(|(name, member, deps, has_lib)| {
                let dir = root.join(member).display().to_string().replace('\\', "/");
                ((*name).to_string(), dir, *deps, *has_lib)
            })
            .collect();
        let borrowed: Vec<(&str, &str, &[&str], bool)> = rebased
            .iter()
            .map(|(name, dir, deps, has_lib)| (name.as_str(), dir.as_str(), *deps, *has_lib))
            .collect();
        workspace_json(&borrowed)
    }

    /// A one-member workspace was previously accepted whatever it
    /// contained, because resolution short-circuited on package count
    /// before ever checking that it looked like a game.
    #[test]
    fn a_lone_non_game_package_is_not_accepted_as_the_game() {
        let root = scratch("lone", &[("tools", false)]);
        let m = meta(&json_at(&root, &[("tools", "tools", &["clap"], true)]));
        assert!(
            matches!(
                m.resolve_package(None),
                Err(ResolveError::NoBevyPackage { .. })
            ),
            "a clap CLI is not a bevy game"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The game crate commonly gets bevy transitively through a sibling,
    /// so the crate that merely names bevy must not outrank the one that
    /// actually declares the plugin.
    #[test]
    fn a_member_with_the_plugin_wins_over_one_that_only_names_bevy() {
        let root = scratch("trans", &[("core", false), ("app", true)]);
        let m = meta(&json_at(
            &root,
            &[
                ("core", "core", &["bevy"], true),
                ("app", "app", &["core"], true),
            ],
        ));
        assert_eq!(m.resolve_package(None).unwrap().name, "app");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// When both signals point at the same member there is nothing to
    /// ask about, even though several members use bevy.
    #[test]
    fn both_signals_agreeing_resolves_an_otherwise_ambiguous_workspace() {
        let root = scratch("agree", &[("client", true), ("server", false)]);
        let m = meta(&json_at(
            &root,
            &[
                ("client", "client", &["bevy"], true),
                ("server", "server", &["bevy"], true),
            ],
        ));
        assert_eq!(m.resolve_package(None).unwrap().name, "client");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cargo_diagnostics_survive_into_the_error() {
        let stderr =
            "error: failed to parse manifest at `/ws/Cargo.toml`\n\n  --> Cargo.toml:4:1\n";
        let line = first_error_line(stderr);
        assert!(line.starts_with("error: failed to parse manifest"));
        assert!(line.contains("Cargo.toml:4:1"));
    }

    #[test]
    fn a_missing_manifest_reports_cargo_rather_than_a_bare_failure() {
        let dir = std::env::temp_dir().join(format!("jackdaw_meta_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let err = resolve_project_package(&dir, None).unwrap_err();
        let ResolveError::NoMetadata { detail, .. } = &err else {
            panic!("expected NoMetadata, got {err:?}");
        };
        assert!(!detail.is_empty(), "cargo's own message should be kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lib_package_keeps_working_for_single_package_projects() {
        let m = meta(&workspace_json(&[("my-game", "/ws", &["bevy"], true)]));
        assert_eq!(
            m.lib_package(),
            Some(("my-game".to_string(), "my_game".to_string()))
        );
        assert_eq!(m.lib_name().as_deref(), Some("my_game"));
    }
}
