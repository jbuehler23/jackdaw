//! Whether a game project carries the `jackdaw_runtime` wiring that
//! component discovery and the editor's Play button depend on.
//!
//! The editor learns a game's types by running its binary with the schema
//! flag, which `JackdawPlugin` answers, and Play drives the same binary over
//! the link the runtime's `pie` feature installs, rendering into the editor
//! once `maybe_windowless` has swapped out the OS window. A project missing
//! any of the four pieces builds fine and then times out in the editor.

use std::path::{Path, PathBuf};

/// The runtime crate a game depends on.
pub const RUNTIME_CRATE: &str = "jackdaw_runtime";

/// The plugin a game adds so the editor can query and drive it.
pub const RUNTIME_PLUGIN: &str = "JackdawPlugin";

/// The function that lets an embedded Play run without an OS window.
pub const WINDOWLESS_FN: &str = "maybe_windowless";

/// The runtime features a game needs for Play. `pie` is the editor link;
/// `physics` builds colliders for authored brushes and terrain.
pub const RUNTIME_FEATURES: [&str; 2] = ["physics", "pie"];

/// The feature without which Play cannot reach the game at all.
pub const PIE_FEATURE: &str = "pie";

/// How a package declares `jackdaw_runtime`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeDependency {
    /// Not declared.
    #[default]
    Missing,
    /// A non-optional `[dependencies]` entry under its own name.
    Plain,
    /// Declared in a form generated code cannot rely on or an edit cannot
    /// safely extend; the text says which, phrased to follow "declared as".
    Unsupported(&'static str),
}

/// Which parts of the runtime wiring a package has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeWiring {
    /// How the package declares `jackdaw_runtime`.
    pub dependency: RuntimeDependency,
    /// The dependency always enables `pie`, directly or through the workspace.
    pub pie_feature: bool,
    /// One of the package's own features enables `jackdaw_runtime/pie`.
    pub pie_via_feature: bool,
    /// The enclosing workspace declares `jackdaw_runtime` for members to inherit.
    pub workspace_declares: bool,
    /// Some source file in the package names `JackdawPlugin`.
    pub plugin: bool,
    /// Some source file in the package names `maybe_windowless`.
    pub windowless: bool,
    /// The package has a `src/main.rs`.
    pub has_main: bool,
}

impl RuntimeWiring {
    /// Inspect the package at `package_dir`. The plugin and windowless checks
    /// look for those identifiers anywhere in the package's sources.
    pub fn inspect(package_dir: &Path) -> Self {
        let sources = source_files(&package_dir.join("src"));
        let mentions = |ident: &str| {
            sources.iter().any(|path| {
                std::fs::read_to_string(path).is_ok_and(|text| mentions_ident(&text, ident))
            })
        };
        let mut wiring = Self {
            plugin: mentions(RUNTIME_PLUGIN),
            windowless: mentions(WINDOWLESS_FN),
            has_main: package_dir.join("src/main.rs").is_file(),
            ..Self::default()
        };
        read_dependency(package_dir, &mut wiring);
        wiring
    }

    /// Whether `pie` is enabled always or through one of the package's features.
    pub fn pie_available(&self) -> bool {
        self.pie_feature || self.pie_via_feature
    }

    /// Whether every part is in place. A package with no `main.rs` is not
    /// held to the windowless check, since its binary lives elsewhere.
    pub fn complete(&self) -> bool {
        self.dependency == RuntimeDependency::Plain
            && self.pie_available()
            && self.plugin
            && (self.windowless || !self.has_main)
    }
}

/// Whether `source` uses `ident` as an identifier. Comments, doc comments
/// and string literals do not count; a file that does not lex never does.
pub fn mentions_ident(source: &str, ident: &str) -> bool {
    fn walk(stream: proc_macro2::TokenStream, ident: &str) -> bool {
        stream.into_iter().any(|tree| match tree {
            proc_macro2::TokenTree::Ident(found) => found == ident,
            proc_macro2::TokenTree::Group(group) => walk(group.stream(), ident),
            _ => false,
        })
    }
    source
        .parse::<proc_macro2::TokenStream>()
        .is_ok_and(|stream| walk(stream, ident))
}

/// Fill in the dependency fields of `wiring` from the package manifest and
/// its workspace. A `{ workspace = true }` entry adds the features the
/// workspace declares for it.
fn read_dependency(package_dir: &Path, wiring: &mut RuntimeWiring) {
    let workspace_runtime = workspace_manifest(package_dir).and_then(|workspace| {
        workspace
            .get("workspace")?
            .get("dependencies")?
            .get(RUNTIME_CRATE)
            .cloned()
    });
    wiring.workspace_declares = workspace_runtime.is_some();
    let Some(manifest) = read_manifest(&package_dir.join("Cargo.toml")) else {
        return;
    };
    let names_runtime = |key: &str, entry: &toml::Value| {
        key == RUNTIME_CRATE
            || entry.get("package").and_then(toml::Value::as_str) == Some(RUNTIME_CRATE)
    };
    let direct = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .and_then(|deps| deps.iter().find(|(key, entry)| names_runtime(key, entry)));
    let in_target = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .is_some_and(|targets| {
            targets.values().any(|target| {
                target
                    .get("dependencies")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|deps| deps.iter().any(|(key, entry)| names_runtime(key, entry)))
            })
        });
    let Some((key, entry)) = direct else {
        if in_target {
            wiring.dependency = RuntimeDependency::Unsupported("a target-specific dependency");
        }
        return;
    };
    let optional = entry
        .get("optional")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    wiring.dependency = if key.as_str() != RUNTIME_CRATE {
        RuntimeDependency::Unsupported("a renamed dependency")
    } else if optional {
        RuntimeDependency::Unsupported("an optional dependency")
    } else if in_target {
        RuntimeDependency::Unsupported("both a plain and a target-specific dependency")
    } else {
        RuntimeDependency::Plain
    };
    let inherits = entry
        .get("workspace")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    wiring.pie_feature = has_feature(entry, PIE_FEATURE)
        || (inherits
            && workspace_runtime
                .as_ref()
                .is_some_and(|inherited| has_feature(inherited, PIE_FEATURE)));
    let enables = [
        format!("{key}/{PIE_FEATURE}"),
        format!("{key}?/{PIE_FEATURE}"),
    ];
    wiring.pie_via_feature = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .is_some_and(|features| {
            features.values().any(|list| {
                list.as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item.as_str()
                            .is_some_and(|s| enables.iter().any(|e| e == s))
                    })
                })
            })
        });
}

fn has_feature(entry: &toml::Value, feature: &str) -> bool {
    entry
        .get("features")
        .and_then(toml::Value::as_array)
        .is_some_and(|features| features.iter().any(|f| f.as_str() == Some(feature)))
}

fn read_manifest(path: &Path) -> Option<toml::Table> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// The manifest of the workspace `package_dir` belongs to: the nearest
/// manifest at or above it with a `[workspace]` table, as cargo finds it.
pub fn workspace_manifest(package_dir: &Path) -> Option<toml::Table> {
    package_dir.ancestors().find_map(|dir| {
        read_manifest(&dir.join("Cargo.toml")).filter(|manifest| manifest.contains_key("workspace"))
    })
}

/// Every `.rs` file under `dir`.
pub fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_runtime_wiring_{name}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
        for (path, text) in files {
            std::fs::write(dir.join("src").join(path), text).unwrap();
        }
        dir
    }

    #[test]
    fn comments_and_strings_do_not_count_as_wiring() {
        let source = "/// Add JackdawPlugin here.\n// maybe_windowless\nconst S: &str = \"JackdawPlugin\";\n";
        assert!(!mentions_ident(source, RUNTIME_PLUGIN));
        assert!(!mentions_ident(source, WINDOWLESS_FN));
        assert!(mentions_ident(
            "fn f(app: &mut App) { app.add_plugins(jackdaw_runtime::JackdawPlugin); }",
            RUNTIME_PLUGIN
        ));
    }

    #[test]
    fn a_wired_package_is_complete() {
        let dir = package(
            "wired",
            "[package]\nname = \"g\"\n\n[dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] }\n",
            &[
                (
                    "lib.rs",
                    "fn b(app: &mut App) { app.add_plugins(JackdawPlugin); }\n",
                ),
                (
                    "main.rs",
                    "fn main() { let p = maybe_windowless(DefaultPlugins); }\n",
                ),
            ],
        );
        assert!(RuntimeWiring::inspect(&dir).complete());
    }

    #[test]
    fn a_dependency_without_pie_is_reported() {
        let dir = package(
            "nopie",
            "[package]\nname = \"g\"\n\n[dependencies]\njackdaw_runtime = \"0.19\"\n",
            &[],
        );
        let wiring = RuntimeWiring::inspect(&dir);
        assert_eq!(wiring.dependency, RuntimeDependency::Plain);
        assert!(!wiring.pie_feature);
        assert!(!wiring.plugin && !wiring.windowless);
    }

    #[test]
    fn pie_inherited_from_the_workspace_counts() {
        let root = package(
            "wspie",
            "[workspace]\nmembers = [\"game\"]\n\n[workspace.dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] }\n",
            &[],
        );
        let member = root.join("game");
        std::fs::create_dir_all(member.join("src")).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"game\"\n\n[dependencies]\n\
             jackdaw_runtime = { workspace = true }\n",
        )
        .unwrap();
        let wiring = RuntimeWiring::inspect(&member);
        assert_eq!(wiring.dependency, RuntimeDependency::Plain);
        assert!(wiring.pie_feature && wiring.workspace_declares);
    }

    #[test]
    fn optional_renamed_and_target_specific_runtimes_are_unsupported() {
        for (name, deps) in [
            (
                "optional",
                "[dependencies]\njackdaw_runtime = { version = \"0.19\", optional = true }\n",
            ),
            (
                "renamed",
                "[dependencies]\nruntime = { package = \"jackdaw_runtime\", version = \"0.19\" }\n",
            ),
            (
                "target",
                "[target.'cfg(unix)'.dependencies]\njackdaw_runtime = \"0.19\"\n",
            ),
        ] {
            let dir = package(name, &format!("[package]\nname = \"g\"\n\n{deps}"), &[]);
            assert!(
                matches!(
                    RuntimeWiring::inspect(&dir).dependency,
                    RuntimeDependency::Unsupported(_)
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn pie_enabled_through_a_package_feature_counts() {
        for spelling in ["jackdaw_runtime/pie", "jackdaw_runtime?/pie"] {
            let dir = package(
                "featurepie",
                &format!(
                    "[package]\nname = \"g\"\n\n[features]\neditor = [\"{spelling}\"]\n\n\
                     [dependencies]\njackdaw_runtime = \"0.19\"\n"
                ),
                &[],
            );
            let wiring = RuntimeWiring::inspect(&dir);
            assert!(!wiring.pie_feature && wiring.pie_via_feature, "{spelling}");
            assert!(wiring.pie_available());
        }
    }

    #[test]
    fn a_package_without_main_is_not_held_to_the_windowless_check() {
        let dir = package(
            "nomain",
            "[package]\nname = \"g\"\n\n[dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"pie\"] }\n",
            &[(
                "lib.rs",
                "fn b(app: &mut App) { app.add_plugins(JackdawPlugin); }\n",
            )],
        );
        let wiring = RuntimeWiring::inspect(&dir);
        assert!(!wiring.has_main && !wiring.windowless);
        assert!(wiring.complete());
    }
}
