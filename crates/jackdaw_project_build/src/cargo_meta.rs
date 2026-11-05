//! `cargo metadata` parsing: which package in a project (or workspace)
//! the editor should build, read without any editor or bevy dependency
//! so the build pipeline and the CLI can resolve a project from the
//! filesystem alone.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use jackdaw_env::user_toolchain_command;
use serde::Deserialize;

/// Where the engine and the editor are developed. A package from one of these
/// is theirs however it is named or wherever it is checked out, forks included.
const ENGINE_REPOSITORIES: [&str; 2] = [
    "https://github.com/bevyengine/bevy",
    "https://github.com/jbuehler23/jackdaw",
];

/// A project's cargo metadata: packages and their targets.
#[derive(Deserialize)]
pub struct CargoMeta {
    packages: Vec<MetaPackage>,
    #[serde(default)]
    workspace_root: String,
    #[serde(default)]
    workspace_members: Vec<String>,
    /// The dependency graph, absent from `--no-deps` output.
    #[serde(default)]
    resolve: Option<MetaResolve>,
}

#[derive(Deserialize)]
struct MetaResolve {
    nodes: Vec<MetaNode>,
}

#[derive(Deserialize)]
struct MetaNode {
    id: String,
    dependencies: Vec<String>,
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
    #[serde(default)]
    repository: Option<String>,
    /// Where the package comes from; absent for a local one.
    #[serde(default)]
    source: Option<String>,
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

impl MetaPackage {
    fn belongs_to_engine(&self) -> bool {
        self.repository.as_deref().is_some_and(|repository| {
            let repository = repository.trim_end_matches('/').trim_end_matches(".git");
            ENGINE_REPOSITORIES.contains(&repository)
        })
    }

    /// The crate names this package's code appears under in a type path.
    fn crate_names(&self) -> impl Iterator<Item = String> + '_ {
        self.targets
            .iter()
            .filter(|target| {
                target.kind.iter().any(|kind| {
                    matches!(
                        kind.as_str(),
                        "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" | "bin"
                    )
                })
            })
            .map(|target| target.name.replace('-', "_"))
    }
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

    /// The metadata with its dependency graph, leaving the lockfile alone.
    /// `None` when the lockfile is missing or out of date, or a package the graph
    /// needs cannot be fetched. May download packages a build does not need.
    fn load_resolved(project_dir: &Path) -> Option<Self> {
        let out = user_toolchain_command("cargo")
            .current_dir(project_dir)
            .args(["metadata", "--locked", "--format-version", "1"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Self::parse(&String::from_utf8_lossy(&out.stdout)).ok()
    }

    /// As [`CargoMeta::load`], keeping cargo's diagnostics so an import
    /// can tell the user why their manifest was rejected. Runs under the
    /// project's own toolchain, which need not be the editor's.
    pub fn load_or_error(project_dir: &Path) -> Result<Self, ResolveError> {
        let fail = |detail: String| ResolveError::NoMetadata {
            root: project_dir.to_path_buf(),
            detail,
        };
        let out = user_toolchain_command("cargo")
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

    /// The crates holding the project's own code: `game`, the other workspace
    /// members and the local packages they reach, stopping at packages of the
    /// engine and the editor.
    ///
    /// Local packages come from the resolved graph, which also sees `[patch]`
    /// entries. Metadata without one yields the workspace members alone.
    fn crates_of_project(&self, game_package: &str, game_crate: &str) -> Vec<String> {
        let mut names = vec![game_crate.to_string()];
        let mut pending: Vec<&str> = self
            .packages
            .iter()
            .filter(|package| self.is_member(package))
            .map(|package| package.id.as_str())
            .collect();
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(package) = self.packages.iter().find(|package| package.id == id) else {
                continue;
            };
            if package.source.is_some()
                || (package.belongs_to_engine() && package.name != game_package)
            {
                continue;
            }
            for name in package.crate_names() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            if let Some(node) = self
                .resolve
                .iter()
                .flat_map(|resolve| &resolve.nodes)
                .find(|node| node.id == id)
            {
                pending.extend(node.dependencies.iter().map(String::as_str));
            }
        }
        names.sort_by(|a, b| {
            (a != game_crate)
                .cmp(&(b != game_crate))
                .then_with(|| a.cmp(b))
        });
        names
    }

    fn is_member(&self, package: &MetaPackage) -> bool {
        self.workspace_members.is_empty()
            || package.id.is_empty()
            || self.workspace_members.contains(&package.id)
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
    let meta = CargoMeta::load_or_error(root)?;
    let mut package = meta.resolve_package(preferred)?;
    package.dir = spelled_under(root, package.dir);
    Ok(package)
}

/// The crates holding the code of the project at `root`, whose game is
/// `game_package`. Asked after a game build, when the lockfile is settled. May
/// reach the network; [`workspace_crates`] does not.
pub fn project_crates(root: &Path, game_package: &str, game_crate: &str) -> Vec<String> {
    let resolved = CargoMeta::load_resolved(root);
    if resolved.is_none() {
        tracing::warn!(
            "could not read the dependency graph of {}; only its workspace crates count as \
             project code",
            root.display()
        );
    }
    resolved
        .map(|meta| meta.crates_of_project(game_package, game_crate))
        .unwrap_or_else(|| workspace_crates(root, game_package, game_crate))
}

/// [`project_crates`] from the workspace members alone, without the network or
/// the lockfile.
pub fn workspace_crates(root: &Path, game_package: &str, game_crate: &str) -> Vec<String> {
    CargoMeta::load(root).map_or_else(
        || vec![game_crate.to_string()],
        |meta| meta.crates_of_project(game_package, game_crate),
    )
}

/// Whether cargo can resolve the full dependency graph of the project at
/// `root`, reporting its first error line. Unlike the `--no-deps` call that
/// finds the package, this one does the version solving, which is where an
/// unpublished or mistyped requirement surfaces.
pub fn resolve_dependencies(root: &Path) -> Result<(), String> {
    let output = user_toolchain_command("cargo")
        .current_dir(root)
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .unwrap_or("cargo could not resolve this project's dependencies")
        .to_string())
}

/// `dir` spelled under `root` the way the caller spelled `root`.
///
/// Cargo reports resolved paths, so under a symlinked root (a macOS temp dir
/// under `/var` is one) the package folder would never compare equal to
/// paths the caller builds from `root`, and a single package would read as a
/// member of a workspace elsewhere.
fn spelled_under(root: &Path, dir: PathBuf) -> PathBuf {
    if dir.starts_with(root) {
        return dir;
    }
    let Ok(resolved_root) = dunce::canonicalize(root) else {
        return dir;
    };
    match dir.strip_prefix(&resolved_root) {
        Ok(rest) if rest.as_os_str().is_empty() => root.to_path_buf(),
        Ok(rest) => root.join(rest),
        Err(_) => dir,
    }
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
    use path_slash::PathExt as _;

    #[cfg(unix)]
    #[test]
    fn a_package_under_a_symlinked_root_keeps_the_callers_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("game")).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real = dunce::canonicalize(&real).unwrap();

        assert_eq!(spelled_under(&link, real.clone()), link);
        assert_eq!(spelled_under(&link, real.join("game")), link.join("game"));
        assert_eq!(spelled_under(&link, link.join("game")), link.join("game"));
        let elsewhere = tmp.path().join("elsewhere");
        assert_eq!(spelled_under(&link, elsewhere.clone()), elsewhere);
    }

    fn meta(json: &str) -> CargoMeta {
        CargoMeta::parse(json).expect("metadata parses")
    }

    fn crates_of_game(packages: &[(&str, &str, &[&str], bool)]) -> Vec<String> {
        let meta = meta(&workspace_json(packages));
        meta.crates_of_project("bevy-jam", "bevy_jam")
    }

    #[test]
    fn every_workspace_member_is_a_project_crate() {
        let crates = crates_of_game(&[
            ("bevy-jam", "/w/game", &["bevy"], true),
            ("bevy_jam_core", "/w/core", &["bevy"], true),
        ]);
        assert_eq!(crates, ["bevy_jam", "bevy_jam_core"]);
    }

    fn package(name: &str, repository: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "id": format!("id-{name}"),
            "manifest_path": format!("/w/{name}/Cargo.toml"),
            "repository": repository,
            "targets": [{ "name": name.replace('-', "_"), "kind": ["lib"] }],
            "dependencies": [],
        })
    }

    fn workspace_of(packages: Vec<serde_json::Value>) -> CargoMeta {
        let members: Vec<&serde_json::Value> = packages.iter().map(|p| &p["id"]).collect();
        let json = serde_json::json!({
            "packages": packages,
            "workspace_root": "/w",
            "workspace_members": members,
        });
        meta(&json.to_string())
    }

    #[test]
    fn engine_and_editor_members_are_told_apart_by_repository_not_name() {
        let meta = workspace_of(vec![
            package("bevy-jam", None),
            package("bevy_jam_core", Some("https://github.com/someone/jam")),
            package("jackdaw_jam_tools", None),
            package("bevy_ui", Some("https://github.com/bevyengine/bevy")),
            package("bevy", Some("https://github.com/bevyengine/bevy/")),
            package("jackdaw_api", Some("https://github.com/jbuehler23/jackdaw")),
            package(
                "jackdaw_runtime",
                Some("https://github.com/jbuehler23/jackdaw.git"),
            ),
        ]);
        assert_eq!(
            meta.crates_of_project("bevy-jam", "bevy_jam"),
            ["bevy_jam", "bevy_jam_core", "jackdaw_jam_tools"]
        );
    }

    #[test]
    fn a_game_inside_the_engine_s_repository_still_counts() {
        let meta = workspace_of(vec![package(
            "bevy-jam",
            Some("https://github.com/bevyengine/bevy"),
        )]);
        assert_eq!(meta.crates_of_project("bevy-jam", "bevy_jam"), ["bevy_jam"]);
    }

    #[test]
    fn every_library_and_binary_name_is_listed() {
        let mut game = package("bevy-jam", None);
        game["targets"] = serde_json::json!([
            { "name": "jam_logic", "kind": ["cdylib"] },
            { "name": "bevy-game", "kind": ["bin"] },
            { "name": "a_demo", "kind": ["example"] },
            { "name": "a_test", "kind": ["test"] },
        ]);
        let meta = workspace_of(vec![game]);
        let crates = meta.crates_of_project("bevy-jam", "bevy_jam");
        assert!(crates.contains(&"jam_logic".to_string()), "got {crates:?}");
        assert!(crates.contains(&"bevy_game".to_string()), "got {crates:?}");
        assert!(!crates.contains(&"a_demo".to_string()), "got {crates:?}");
        assert!(!crates.contains(&"a_test".to_string()), "got {crates:?}");
    }

    #[test]
    fn local_packages_in_the_graph_count_and_engine_ones_end_the_walk() {
        let game = package("bevy-jam", None);
        let mut registry = package("serde", None);
        registry["source"] =
            serde_json::json!("registry+https://github.com/rust-lang/crates.io-index");
        let bevy = package("bevy", Some("https://github.com/bevyengine/bevy"));
        let json = serde_json::json!({
            "packages": [
                game,
                package("bevy_mylib", None),
                package("jackdaw_shared", None),
                registry,
                bevy,
                package("bevy_ecs_macros", None),
            ],
            "workspace_root": "/w",
            "workspace_members": ["id-bevy-jam"],
            "resolve": { "nodes": [
                { "id": "id-bevy-jam", "dependencies": ["id-bevy_mylib", "id-bevy", "id-serde"] },
                { "id": "id-bevy_mylib", "dependencies": ["id-jackdaw_shared", "id-bevy-jam"] },
                { "id": "id-jackdaw_shared", "dependencies": [] },
                { "id": "id-bevy", "dependencies": ["id-bevy_ecs_macros"] },
            ]},
        });
        let meta = meta(&json.to_string());
        assert_eq!(
            meta.crates_of_project("bevy-jam", "bevy_jam"),
            ["bevy_jam", "bevy_mylib", "jackdaw_shared"]
        );
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
                let dir = root.join(member).to_slash_lossy().into_owned();
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
