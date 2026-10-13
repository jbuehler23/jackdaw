//! The registry of installed editors, `installs.json`, and how a project's
//! editor pin picks one of them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::index::{Channel, Release};
use crate::version;

/// The registry's file name inside the launcher directory.
pub const REGISTRY_FILE: &str = "installs.json";

/// The directory inside the launcher directory that holds one directory
/// per installed editor, named by its version.
pub const EDITORS_DIR: &str = "editors";

/// The editor binary's file name inside an install.
pub const EDITOR_EXECUTABLE: &str = if cfg!(windows) {
    "jackdaw.exe"
} else {
    "jackdaw"
};

/// The registry layout this build reads and writes.
const SCHEMA: u32 = 1;

/// One installed editor.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// `0.20.1`, or `nightly-YYYY-MM-DD` on Bleeding Edge.
    pub version: String,
    pub channel: Channel,
    /// The Bevy minor it targets, or `main`.
    pub bevy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bevy_rev: Option<String>,
    /// The directory holding the editor binary.
    pub path: PathBuf,
    /// Installed by `jd`, which may delete it. An install registered with
    /// `jd editor locate` is only ever unregistered.
    #[serde(default)]
    pub managed: bool,
    /// Optional editor modules installed with it, by short name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// A nightly the index marked as a known-good weekly build.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub known_good: bool,
}

impl Install {
    /// A managed install of `release` at `path`.
    pub fn from_release(release: &Release, path: PathBuf) -> Self {
        Self {
            version: release.version.clone(),
            channel: release.channel,
            bevy: release.bevy.clone(),
            bevy_rev: release.bevy_rev.clone(),
            path,
            managed: true,
            modules: Vec::new(),
            known_good: release.known_good,
        }
    }

    /// The editor binary.
    pub fn executable(&self) -> PathBuf {
        self.path.join(EDITOR_EXECUTABLE)
    }
}

/// The editor a project asks for, from the `[jackdaw]` table of its
/// `jackdaw.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorPin {
    /// The editor version the project was last saved with.
    pub version: Option<String>,
    /// `stable` when absent.
    pub channel: Option<Channel>,
    /// The nightly a Bleeding Edge project uses.
    pub build: Option<String>,
}

impl EditorPin {
    /// Build a pin from the table's text fields.
    pub fn from_fields(
        version: Option<&str>,
        channel: Option<&str>,
        build: Option<&str>,
    ) -> Result<Self, String> {
        let channel = channel
            .map(|name| {
                Channel::parse(name).ok_or_else(|| {
                    format!("unknown channel `{name}`; expected stable or bleeding-edge")
                })
            })
            .transpose()?;
        Ok(Self {
            version: version.map(str::to_string),
            channel,
            build: build.map(str::to_string),
        })
    }
}

/// Which install a pin selects.
#[derive(Debug, PartialEq, Eq)]
pub enum PinResolution<'a> {
    /// The project names no editor.
    Unpinned,
    /// The newest install of the pinned line, or the pinned nightly.
    Installed {
        install: &'a Install,
        /// Only an older patch of the line is installed.
        older_than_pin: bool,
    },
    /// Nothing installed matches; `jd editor install <install_query>` fixes it.
    Missing { install_query: String },
}

/// The whole `installs.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Installs {
    pub schema: u32,
    /// The version opened for projects that pin no editor.
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub installs: Vec<Install>,
    /// Each project directory opened through `jd`, and the install it
    /// opened with.
    #[serde(default)]
    pub projects: BTreeMap<PathBuf, String>,
}

impl Default for Installs {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            default: None,
            installs: Vec::new(),
            projects: BTreeMap::new(),
        }
    }
}

impl Installs {
    /// Read the registry at `path`; a missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let installs: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if installs.schema > SCHEMA {
            return Err(format!(
                "{} was written by a newer jd; update jd",
                path.display()
            ));
        }
        Ok(installs)
    }

    /// Write the registry to `path`, replacing the old file in one rename
    /// so a reader never sees half of it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        crate::write_atomically(path, text.as_bytes())
    }

    /// The install of exactly this version.
    pub fn get(&self, version: &str) -> Option<&Install> {
        self.installs
            .iter()
            .find(|install| install.version == version)
    }

    /// Mutable access to the install of exactly this version.
    pub fn get_mut(&mut self, version: &str) -> Option<&mut Install> {
        self.installs
            .iter_mut()
            .find(|install| install.version == version)
    }

    /// Register `install`, replacing one of the same version. The first
    /// stable install becomes the default.
    pub fn add(&mut self, install: Install) {
        self.installs
            .retain(|known| known.version != install.version);
        if self.default.is_none() && install.channel == Channel::Stable {
            self.default = Some(install.version.clone());
        }
        self.installs.push(install);
        self.installs
            .sort_by(|a, b| version::compare(&b.version, &a.version));
    }

    /// Unregister a version and forget the projects that opened with it.
    /// When it was the default, the newest remaining stable install takes
    /// over.
    pub fn remove(&mut self, version: &str) -> Option<Install> {
        let position = self
            .installs
            .iter()
            .position(|install| install.version == version)?;
        let removed = self.installs.remove(position);
        self.projects.retain(|_, pinned| pinned != version);
        if self.default.as_deref() == Some(version) {
            self.default = self
                .installs
                .iter()
                .filter(|install| install.channel == Channel::Stable)
                .max_by(|a, b| version::compare(&a.version, &b.version))
                .map(|install| install.version.clone());
        }
        Some(removed)
    }

    /// Make `version` the default editor.
    pub fn set_default(&mut self, version: &str) -> Result<(), String> {
        if self.get(version).is_none() {
            return Err(format!("{version} is not installed"));
        }
        self.default = Some(version.to_string());
        Ok(())
    }

    /// The default editor, if one is set and still installed.
    pub fn default_install(&self) -> Option<&Install> {
        self.default
            .as_deref()
            .and_then(|version| self.get(version))
    }

    /// Remember that `project` opened with `version`.
    pub fn record_project(&mut self, project: &Path, version: &str) {
        self.projects
            .insert(project.to_path_buf(), version.to_string());
    }

    /// Projects that still exist and opened with `version` last.
    pub fn pinning_projects(&self, version: &str) -> Vec<&Path> {
        self.projects
            .iter()
            .filter(|(project, pinned)| *pinned == version && project.is_dir())
            .map(|(project, _)| project.as_path())
            .collect()
    }

    /// The install a project's pin selects.
    ///
    /// A stable pin opens the newest installed patch of its line, so a
    /// project saved with `0.20.1` opens in `0.20.3` once that is
    /// installed. A Bleeding Edge pin needs its exact nightly.
    pub fn resolve_pin(&self, pin: &EditorPin) -> PinResolution<'_> {
        if pin.channel == Some(Channel::BleedingEdge) {
            let Some(build) = pin.build.as_deref().or(pin.version.as_deref()) else {
                return PinResolution::Unpinned;
            };
            return match self.get(build) {
                Some(install) => PinResolution::Installed {
                    install,
                    older_than_pin: false,
                },
                None => PinResolution::Missing {
                    install_query: build.to_string(),
                },
            };
        }
        let Some(pinned) = pin.version.as_deref() else {
            return PinResolution::Unpinned;
        };
        let Some(line) = version::line_of(pinned) else {
            return match self.get(pinned) {
                Some(install) => PinResolution::Installed {
                    install,
                    older_than_pin: false,
                },
                None => PinResolution::Missing {
                    install_query: pinned.to_string(),
                },
            };
        };
        let newest = self
            .installs
            .iter()
            .filter(|install| {
                install.channel == Channel::Stable
                    && version::line_of(&install.version).as_deref() == Some(line.as_str())
            })
            .max_by(|a, b| version::compare(&a.version, &b.version));
        match newest {
            Some(install) => PinResolution::Installed {
                install,
                older_than_pin: version::compare(&install.version, pinned).is_lt(),
            },
            None => PinResolution::Missing {
                install_query: line,
            },
        }
    }

    /// Bleeding Edge installs `jd editor clean` removes: every managed
    /// nightly except the newest, the default, known-good builds and those
    /// a project still opens with.
    pub fn removable_nightlies(&self) -> Vec<&Install> {
        let nightlies = || {
            self.installs
                .iter()
                .filter(|install| install.channel == Channel::BleedingEdge)
        };
        let newest = nightlies()
            .max_by(|a, b| version::compare(&a.version, &b.version))
            .map(|install| install.version.clone());
        nightlies()
            .filter(|install| {
                install.managed
                    && !install.known_good
                    && Some(&install.version) != newest.as_ref()
                    && self.default.as_ref() != Some(&install.version)
                    && self.pinning_projects(&install.version).is_empty()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(version: &str) -> Install {
        let nightly = version::is_nightly(version);
        Install {
            version: version.to_string(),
            channel: if nightly {
                Channel::BleedingEdge
            } else {
                Channel::Stable
            },
            bevy: version::line_of(version).unwrap_or_else(|| "main".into()),
            bevy_rev: None,
            path: PathBuf::from("/editors").join(version),
            managed: true,
            modules: Vec::new(),
            known_good: false,
        }
    }

    fn registry(versions: &[&str]) -> Installs {
        let mut installs = Installs::default();
        for version in versions {
            installs.add(install(version));
        }
        installs
    }

    fn stable_pin(version: &str) -> EditorPin {
        EditorPin::from_fields(Some(version), None, None).unwrap()
    }

    fn resolved(installs: &Installs, pin: &EditorPin) -> Option<(String, bool)> {
        match installs.resolve_pin(pin) {
            PinResolution::Installed {
                install,
                older_than_pin,
            } => Some((install.version.clone(), older_than_pin)),
            _ => None,
        }
    }

    #[test]
    fn a_stable_pin_opens_the_newest_installed_patch_of_its_line() {
        let installs = registry(&["0.19.4", "0.20.1", "0.20.3", "0.21.0"]);
        assert_eq!(
            resolved(&installs, &stable_pin("0.20.1")),
            Some(("0.20.3".into(), false))
        );
        assert_eq!(
            resolved(&installs, &stable_pin("0.19.0")),
            Some(("0.19.4".into(), false))
        );
    }

    #[test]
    fn an_older_patch_opens_but_is_reported_as_older() {
        let installs = registry(&["0.20.0"]);
        assert_eq!(
            resolved(&installs, &stable_pin("0.20.2")),
            Some(("0.20.0".into(), true))
        );
    }

    #[test]
    fn a_missing_line_names_the_line_to_install() {
        let installs = registry(&["0.19.4"]);
        assert_eq!(
            installs.resolve_pin(&stable_pin("0.20.1")),
            PinResolution::Missing {
                install_query: "0.20".into()
            }
        );
        assert_eq!(
            installs.resolve_pin(&EditorPin::default()),
            PinResolution::Unpinned
        );
    }

    #[test]
    fn a_bleeding_edge_pin_needs_its_exact_nightly() {
        let installs = registry(&["nightly-2026-10-11", "0.20.1"]);
        let pin = |build: &str| {
            EditorPin::from_fields(Some("0.21.0-dev"), Some("bleeding-edge"), Some(build)).unwrap()
        };
        assert_eq!(
            resolved(&installs, &pin("nightly-2026-10-11")),
            Some(("nightly-2026-10-11".into(), false))
        );
        assert_eq!(
            installs.resolve_pin(&pin("nightly-2026-10-09")),
            PinResolution::Missing {
                install_query: "nightly-2026-10-09".into()
            }
        );
        assert!(EditorPin::from_fields(None, Some("beta"), None).is_err());
    }

    #[test]
    fn the_first_stable_install_becomes_the_default_and_removal_hands_it_on() {
        let mut installs = registry(&["nightly-2026-10-11", "0.19.4", "0.20.1"]);
        assert_eq!(installs.default.as_deref(), Some("0.19.4"));
        installs.set_default("0.20.1").unwrap();
        assert!(installs.set_default("0.18.0").is_err());
        installs.remove("0.20.1");
        assert_eq!(installs.default.as_deref(), Some("0.19.4"));
        installs.remove("0.19.4");
        assert_eq!(installs.default, None);
    }

    #[test]
    fn clean_keeps_the_newest_known_good_default_and_pinned_nightlies() {
        let project = tempfile::tempdir().unwrap();
        let mut installs = registry(&[
            "nightly-2026-10-01",
            "nightly-2026-10-02",
            "nightly-2026-10-03",
            "nightly-2026-10-04",
            "nightly-2026-10-05",
            "nightly-2026-10-06",
            "0.20.1",
        ]);
        installs.get_mut("nightly-2026-10-02").unwrap().known_good = true;
        installs.get_mut("nightly-2026-10-04").unwrap().managed = false;
        installs.record_project(project.path(), "nightly-2026-10-03");
        installs.record_project(Path::new("/gone/project"), "nightly-2026-10-05");
        installs.default = Some("nightly-2026-10-01".into());

        let removable: Vec<_> = installs
            .removable_nightlies()
            .iter()
            .map(|install| install.version.as_str())
            .collect();
        assert_eq!(removable, ["nightly-2026-10-05"]);
    }

    #[test]
    fn the_registry_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(REGISTRY_FILE);
        assert_eq!(Installs::load(&path).unwrap(), Installs::default());

        let mut installs = registry(&["0.20.1"]);
        installs.get_mut("0.20.1").unwrap().modules = vec!["physics".into()];
        installs.record_project(Path::new("/projects/game"), "0.20.1");
        installs.save(&path).unwrap();
        assert_eq!(Installs::load(&path).unwrap(), installs);
    }
}
