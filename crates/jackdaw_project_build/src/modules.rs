//! The optional jackdaw modules a project can use.
//!
//! A module pairs editor tooling with what the game needs for it: features
//! of `jackdaw_runtime` and any crates of its own. The catalog is data
//! (`modules.json`), read by the launcher, `jd`, the project settings and
//! the inspector alike, so a module is described in one place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::runtime_wiring::RUNTIME_CRATE;

/// The ids of a set of modules, as `[modules]` in `jackdaw.toml` names them.
pub type ModuleSet = BTreeSet<String>;

/// One optional module.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    /// The key under `[modules]` in `jackdaw.toml`.
    pub id: String,
    /// The name people see.
    pub name: String,
    /// One line on what the module does.
    pub description: String,
    /// Whether a new game starts with it.
    #[serde(default, rename = "default")]
    pub default_enabled: bool,
    /// Features of `jackdaw_runtime` the game enables for it.
    #[serde(default)]
    pub runtime_features: Vec<String>,
    /// Features of `jackdaw_runtime` an older game enabled for it, which no
    /// longer exist but still show the game uses the module.
    #[serde(default)]
    pub former_runtime_features: Vec<String>,
    /// Crates the game depends on for it, besides `jackdaw_runtime`.
    #[serde(default)]
    pub crates: Vec<ModuleCrate>,
    /// Crates whose types the module's editor tooling handles.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Type path prefixes of the components the module's crates add, which
    /// the inspector offers as the project's own rather than as editor
    /// internals.
    #[serde(default)]
    pub component_prefixes: Vec<String>,
    /// The heading the component picker groups those components under,
    /// instead of their crate's.
    #[serde(default)]
    pub component_group: Option<String>,
    /// Components of the module's crates the picker leaves out, such as a
    /// solver's own state. An entry ending in `::` hides everything under it.
    #[serde(default)]
    pub hidden_components: Vec<String>,
    /// Modules this one needs.
    #[serde(default)]
    pub requires: Vec<String>,
    /// The id of the editor extension that carries the module's tooling,
    /// for a module whose tooling is not built into the editor.
    #[serde(default)]
    pub extension: Option<String>,
    /// Why a new game cannot use the module on this editor line yet, such as
    /// a crate it needs having no release for this Bevy.
    #[serde(default)]
    pub unavailable: Option<String>,
}

/// A crate a module adds to the game.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModuleCrate {
    /// The crate's name.
    pub name: String,
    /// The version requirement. Absent for a jackdaw crate, which comes from
    /// wherever the project's `jackdaw_runtime` does.
    #[serde(default)]
    pub version: Option<String>,
    /// A git repository the crate comes from instead of the registry, for a
    /// crate with no release on jackdaw's Bevy yet.
    #[serde(default)]
    pub git: Option<String>,
    /// The commit of [`Self::git`] to build.
    #[serde(default)]
    pub rev: Option<String>,
    /// The plugin a new game adds to its `App` for it, as an expression.
    #[serde(default)]
    pub plugin: Option<String>,
}

impl ModuleCrate {
    /// How a game's `[dependencies]` asks for this crate, or `None` for a
    /// jackdaw crate, which follows the project's `jackdaw_runtime`.
    pub fn requirement(&self) -> Option<String> {
        let Some(git) = &self.git else {
            return self
                .version
                .as_ref()
                .map(|version| format!("\"{version}\""));
        };
        let mut keys = Vec::new();
        if let Some(version) = &self.version {
            keys.push(format!("version = \"{version}\""));
        }
        keys.push(format!("git = \"{git}\""));
        if let Some(rev) = &self.rev {
            keys.push(format!("rev = \"{rev}\""));
        }
        Some(format!("{{ {} }}", keys.join(", ")))
    }
}

/// Every module this jackdaw knows.
#[derive(Clone, Debug)]
pub struct ModuleCatalog {
    modules: Vec<ModuleInfo>,
}

impl ModuleCatalog {
    /// The modules this build ships with.
    pub fn builtin() -> &'static Self {
        static CATALOG: OnceLock<ModuleCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            Self::from_json(include_str!("modules.json"))
                .unwrap_or_else(|error| panic!("the built-in module catalog is invalid: {error}"))
        })
    }

    /// A catalog from its JSON form: an array of modules. Ids must be unique
    /// and every requirement must name a module in the same catalog.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let modules: Vec<ModuleInfo> =
            serde_json::from_str(text).map_err(|error| error.to_string())?;
        let mut seen = BTreeSet::new();
        for module in &modules {
            if !seen.insert(module.id.as_str()) {
                return Err(format!("module `{}` is listed twice", module.id));
            }
        }
        for module in &modules {
            if let Some(missing) = module
                .requires
                .iter()
                .find(|id| !seen.contains(id.as_str()))
            {
                return Err(format!(
                    "module `{}` requires `{missing}`, which is not in the catalog",
                    module.id
                ));
            }
        }
        Ok(Self { modules })
    }

    /// The modules in display order.
    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    /// The module with this id.
    pub fn get(&self, id: &str) -> Option<&ModuleInfo> {
        self.modules.iter().find(|module| module.id == id)
    }

    /// The module whose editor tooling handles types from `crate_name`.
    pub fn providing_crate(&self, crate_name: &str) -> Option<&ModuleInfo> {
        self.modules
            .iter()
            .find(|module| module.provides.iter().any(|name| name == crate_name))
    }

    /// The module whose crates add the component at `type_path`.
    pub fn component_module(&self, type_path: &str) -> Option<&ModuleInfo> {
        self.modules.iter().find(|module| {
            module
                .component_prefixes
                .iter()
                .any(|prefix| type_path.starts_with(prefix.as_str()))
        })
    }

    /// The modules a new game starts with: the default ones a game can use
    /// on this editor line.
    pub fn defaults(&self) -> ModuleSet {
        self.complete(
            self.modules
                .iter()
                .filter(|module| module.default_enabled && module.unavailable.is_none())
                .map(|module| module.id.clone())
                .collect(),
        )
    }

    /// Why module `id` cannot be added to a new game, if it cannot.
    pub fn unavailable(&self, id: &str) -> Option<&str> {
        self.get(id)?.unavailable.as_deref()
    }

    /// `set`, refused when it names a module a new game cannot use yet.
    pub fn available(&self, set: ModuleSet) -> Result<ModuleSet, String> {
        match set.iter().find_map(|id| Some((id, self.unavailable(id)?))) {
            Some((id, reason)) => Err(format!("module `{id}` is not compatible: {reason}")),
            None => Ok(set),
        }
    }

    /// `set` with everything its modules require added.
    pub fn complete(&self, mut set: ModuleSet) -> ModuleSet {
        let mut pending: Vec<String> = set.iter().cloned().collect();
        while let Some(id) = pending.pop() {
            for required in self.get(&id).map_or(&[][..], |module| &module.requires) {
                if set.insert(required.clone()) {
                    pending.push(required.clone());
                }
            }
        }
        set
    }

    /// Turn `id` on in `set`, with what it requires. Returns the modules
    /// turned on along with it.
    pub fn enable(&self, set: &mut ModuleSet, id: &str) -> Vec<String> {
        let before = set.clone();
        let mut single = ModuleSet::new();
        single.insert(id.to_string());
        set.extend(self.complete(single));
        set.difference(&before)
            .filter(|added| added.as_str() != id)
            .cloned()
            .collect()
    }

    /// Turn `id` off in `set`, with every module that requires it. Returns
    /// the modules turned off along with it.
    pub fn disable(&self, set: &mut ModuleSet, id: &str) -> Vec<String> {
        let mut dropped = Vec::new();
        set.remove(id);
        loop {
            let dependent = set
                .iter()
                .find(|other| {
                    self.get(other).is_some_and(|module| {
                        module
                            .requires
                            .iter()
                            .any(|required| !set.contains(required))
                    })
                })
                .cloned();
            let Some(dependent) = dependent else {
                break;
            };
            set.remove(&dependent);
            dropped.push(dependent);
        }
        dropped
    }

    /// The modules a comma-separated list names, with their requirements.
    pub fn parse_list(&self, list: &str) -> Result<ModuleSet, String> {
        let mut set = ModuleSet::new();
        for id in list.split(',').map(str::trim).filter(|id| !id.is_empty()) {
            if self.get(id).is_none() {
                let known: Vec<&str> = self.modules.iter().map(|m| m.id.as_str()).collect();
                return Err(format!(
                    "unknown module `{id}` (known: {})",
                    known.join(", ")
                ));
            }

            set.insert(id.to_string());
        }
        Ok(self.complete(set))
    }

    /// The modules a game's declared dependencies show it already uses.
    pub fn detect(&self, game: &GameDependencies) -> ModuleSet {
        let set = self
            .modules
            .iter()
            .filter(|module| {
                module
                    .runtime_features
                    .iter()
                    .chain(&module.former_runtime_features)
                    .any(|feature| game.runtime_features.contains(feature))
                    || module
                        .crates
                        .iter()
                        .map(|krate| &krate.name)
                        .chain(&module.provides)
                        .any(|name| game.crates.contains(name))
            })
            .map(|module| module.id.clone())
            .collect();
        self.complete(set)
    }

    /// The features of `jackdaw_runtime` that `game` still enables but that
    /// no longer exist, each with the module that took its place.
    pub fn removed_runtime_features<'a>(
        &'a self,
        game: &'a GameDependencies,
    ) -> Vec<(&'a str, &'a ModuleInfo)> {
        game.runtime_features
            .iter()
            .filter_map(|feature| Some((feature.as_str(), self.replacing_feature(feature)?)))
            .collect()
    }

    /// The module that took the place of the removed `jackdaw_runtime`
    /// feature `feature`.
    pub fn replacing_feature(&self, feature: &str) -> Option<&ModuleInfo> {
        self.modules.iter().find(|module| {
            module
                .former_runtime_features
                .iter()
                .any(|former| former == feature)
        })
    }

    /// The display names of `ids` in catalog order, joined for a sentence.
    pub fn names(&self, ids: &[String]) -> String {
        let position = |id: &String| {
            self.modules
                .iter()
                .position(|module| &module.id == id)
                .unwrap_or(usize::MAX)
        };
        let mut ordered: Vec<&String> = ids.iter().collect();
        ordered.sort_by_key(|id| position(id));
        let names: Vec<&str> = ordered
            .into_iter()
            .map(|id| {
                self.get(id)
                    .map_or(id.as_str(), |module| module.name.as_str())
            })
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [only] => (*only).to_string(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    }
}

/// What a game's Cargo manifest declares that bears on modules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameDependencies {
    /// Features the `jackdaw_runtime` dependency enables, including any its
    /// workspace entry adds.
    pub runtime_features: BTreeSet<String>,
    /// Crates listed under `[dependencies]`, by their package names.
    pub crates: BTreeSet<String>,
}

impl GameDependencies {
    /// Read the manifest of the package at `package_dir`. `workspace_root`
    /// is where a `{ workspace = true }` entry is looked up; for a single
    /// package it is the same directory. An unreadable manifest declares
    /// nothing.
    pub fn read(package_dir: &Path, workspace_root: &Path) -> Self {
        let read = |dir: &Path| -> Option<toml::Table> {
            toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml")).ok()?).ok()
        };
        let mut found = Self::default();
        let Some(manifest) = read(package_dir) else {
            return found;
        };
        let workspace_deps = read(workspace_root).and_then(|root| {
            root.get("workspace")?
                .get("dependencies")?
                .as_table()
                .cloned()
        });
        let Some(deps) = manifest.get("dependencies").and_then(toml::Value::as_table) else {
            return found;
        };
        for (key, entry) in deps {
            let name = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            found.crates.insert(name.to_string());
            if name != RUNTIME_CRATE {
                continue;
            }
            let mut add_features = |entry: &toml::Value| {
                let features = entry.get("features").and_then(toml::Value::as_array);
                for feature in features.into_iter().flatten() {
                    if let Some(feature) = feature.as_str() {
                        found.runtime_features.insert(feature.to_string());
                    }
                }
            };
            add_features(entry);
            if entry.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                && let Some(inherited) = workspace_deps.as_ref().and_then(|deps| deps.get(key))
            {
                add_features(inherited);
            }
        }
        found
    }
}

/// The `[modules]` block for a project using `enabled`: every catalog module
/// with whether it is on.
pub fn modules_toml(enabled: &ModuleSet) -> String {
    let mut out = String::from(
        "# Optional jackdaw modules this project uses. Change them from the\n\
         # editor (File > Modules) so the game's Cargo.toml follows.\n\
         [modules]\n",
    );
    for module in ModuleCatalog::builtin().modules() {
        out.push_str(&format!(
            "{} = {}\n",
            module.id,
            enabled.contains(&module.id)
        ));
    }
    out
}

/// `jackdaw_toml` with its `[modules]` table set to `enabled`, leaving the
/// rest of the file as written. Ids the catalog does not know keep whatever
/// value they had.
pub fn with_modules(jackdaw_toml: &str, enabled: &ModuleSet) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut = jackdaw_toml
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    if !doc.contains_key("modules") {
        let rendered = format!(
            "{}{}{}",
            jackdaw_toml.trim_end(),
            if jackdaw_toml.trim().is_empty() {
                ""
            } else {
                "\n\n"
            },
            modules_toml(enabled)
        );
        return Ok(rendered);
    }
    let table = doc["modules"]
        .as_table_mut()
        .ok_or_else(|| "`modules` in jackdaw.toml is not a table".to_string())?;
    for module in ModuleCatalog::builtin().modules() {
        table.insert(&module.id, toml_edit::value(enabled.contains(&module.id)));
    }
    Ok(doc.to_string())
}

/// The modules a `[modules]` table turns on, with their requirements.
pub(crate) fn enabled_in(table: &BTreeMap<String, bool>) -> ModuleSet {
    ModuleCatalog::builtin().complete(
        table
            .iter()
            .filter(|(_, on)| **on)
            .map(|(id, _)| id.clone())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> ModuleSet {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn a_new_game_starts_with_no_module_this_line_cannot_build() {
        let catalog = ModuleCatalog::builtin();
        assert!(catalog.unavailable("physics").is_some());
        assert_eq!(catalog.defaults(), ModuleSet::new());
    }

    #[test]
    fn navmesh_needs_no_other_module() {
        let catalog = ModuleCatalog::builtin();
        let mut modules = ModuleSet::new();
        assert!(catalog.enable(&mut modules, "navmesh").is_empty());
        assert_eq!(modules, set(&["navmesh"]));
    }

    #[test]
    fn a_listed_module_must_be_known_and_available() {
        let catalog = ModuleCatalog::builtin();
        assert_eq!(catalog.parse_list("navmesh").unwrap(), set(&["navmesh"]));
        assert_eq!(catalog.parse_list("").unwrap(), ModuleSet::new());
        assert!(catalog.parse_list("rendering").is_err());
        let physics = catalog.parse_list("physics").unwrap();
        let error = catalog.available(physics).unwrap_err();
        assert!(error.contains("not compatible"), "{error}");
    }

    #[test]
    fn a_catalog_requiring_a_module_it_lacks_is_refused() {
        let error = ModuleCatalog::from_json(
            r#"[{"id": "a", "name": "A", "description": "", "requires": ["b"]}]"#,
        )
        .unwrap_err();
        assert!(error.contains("`b`"), "{error}");
        assert!(
            ModuleCatalog::from_json(
                r#"[{"id": "a", "name": "A", "description": ""},
                    {"id": "a", "name": "A", "description": ""}]"#
            )
            .is_err()
        );
    }

    #[test]
    fn a_component_is_traced_to_the_module_whose_crate_adds_it() {
        let catalog = ModuleCatalog::builtin();
        let module = |path: &str| catalog.component_module(path).map(|m| m.id.as_str());
        assert_eq!(
            module("avian3d::dynamics::rigid_body::RigidBody"),
            Some("physics")
        );
        assert_eq!(
            module("jackdaw_multiplayer::Replication"),
            Some("multiplayer")
        );
        assert_eq!(module("bevy_transform::components::Transform"), None);
    }

    /// A module hides only what its own crates add.
    #[test]
    fn hidden_components_belong_to_their_module() {
        let catalog = ModuleCatalog::builtin();
        for module in catalog.modules() {
            for hidden in &module.hidden_components {
                assert_eq!(
                    catalog.component_module(hidden).map(|m| m.id.as_str()),
                    Some(module.id.as_str()),
                    "{hidden}"
                );
            }
        }
    }

    #[test]
    fn a_crate_is_traced_to_the_module_that_handles_it() {
        let catalog = ModuleCatalog::builtin();
        let module = |name: &str| catalog.providing_crate(name).map(|m| m.id.as_str());
        assert_eq!(module("avian3d"), Some("physics"));
        assert_eq!(module("rerecast"), Some("navmesh"));
        assert_eq!(module("jackdaw_multiplayer"), Some("multiplayer"));
        assert_eq!(module("bevy_hanabi"), None);
    }

    /// The physics module adds the avian its integration is built with; two
    /// avians in one game would not share a `Collider`.
    #[test]
    fn the_physics_module_adds_the_avian_its_integration_uses() {
        let workspace: toml::Table = toml::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../integrations/physics/Cargo.toml"
            ))
            .unwrap(),
        )
        .unwrap();
        let avian = &workspace["workspace"]["dependencies"]["avian3d"];
        let key = |name: &str| avian.get(name).and_then(|v| v.as_str()).map(str::to_string);
        let physics = &ModuleCatalog::builtin().get("physics").unwrap().crates[0];
        assert_eq!(physics.version, key("version"));
        assert_eq!(physics.git, key("git"));
        assert_eq!(physics.rev, key("rev"));
        assert!(physics.requirement().is_some());
    }

    fn package(manifest: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), manifest).unwrap();
        dir
    }

    #[test]
    fn modules_are_detected_from_runtime_features_and_crates() {
        let dir = package(
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"navmesh\", \"pie\"] }\n\
             avian3d = \"0.8\"\n\
             net = { package = \"jackdaw_multiplayer\", version = \"0.19\" }\n",
        );
        let game = GameDependencies::read(dir.path(), dir.path());
        assert_eq!(
            ModuleCatalog::builtin().detect(&game),
            set(&["multiplayer", "navmesh", "physics"])
        );
    }

    #[test]
    fn a_removed_runtime_feature_names_the_module_that_replaced_it() {
        let dir = package(
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"physics\", \"navmesh\", \"pie\"] }\n",
        );
        let game = GameDependencies::read(dir.path(), dir.path());
        let removed: Vec<(&str, &str)> = ModuleCatalog::builtin()
            .removed_runtime_features(&game)
            .into_iter()
            .map(|(feature, module)| (feature, module.id.as_str()))
            .collect();
        assert_eq!(removed, [("physics", "physics")]);
    }

    #[test]
    fn a_workspace_entry_adds_its_runtime_features() {
        let root = package(
            "[workspace]\nmembers = [\"game\"]\n\n[workspace.dependencies]\n\
             jackdaw_runtime = { version = \"0.19\", features = [\"physics\"] }\n",
        );
        let member = root.path().join("game");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n\n[dependencies]\n\
             jackdaw_runtime = { workspace = true, features = [\"pie\"] }\n",
        )
        .unwrap();
        let game = GameDependencies::read(&member, root.path());
        assert_eq!(game.runtime_features, set(&["physics", "pie"]));
        assert_eq!(ModuleCatalog::builtin().detect(&game), set(&["physics"]));
    }

    #[test]
    fn the_modules_table_is_set_in_place_and_added_when_absent() {
        let original = "package = \"g\"\n\n[jackdaw]\nversion = \"0.19.0\"\n\n\
                        [modules]\nphysics = true # needed for the ships\nfuture = true\n";
        let updated = with_modules(original, &set(&["multiplayer"])).unwrap();
        let parsed: toml::Table = toml::from_str(&updated).unwrap();
        let modules = parsed["modules"].as_table().unwrap();
        assert_eq!(modules["physics"].as_bool(), Some(false));
        assert_eq!(modules["multiplayer"].as_bool(), Some(true));
        assert_eq!(modules["future"].as_bool(), Some(true));
        assert!(updated.starts_with("package = \"g\"\n\n[jackdaw]"));

        let added = with_modules("[jackdaw]\nversion = \"0.19.0\"\n", &set(&["physics"])).unwrap();
        let parsed: toml::Table = toml::from_str(&added).unwrap();
        assert_eq!(parsed["modules"]["physics"].as_bool(), Some(true));
        assert_eq!(parsed["modules"]["navmesh"].as_bool(), Some(false));
    }
}
