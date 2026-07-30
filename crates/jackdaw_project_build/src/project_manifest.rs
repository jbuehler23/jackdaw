//! The build pipeline's view of `jackdaw.toml`.
//!
//! Only the keys a build needs: which package to build, which plugin
//! type to install, and the version pins written when the project was
//! set up. Run configurations live in the same file but are the
//! editor's concern and are parsed there (`jackdaw_pie_protocol`);
//! unknown keys are ignored on both sides, so the two readers coexist.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// File name at the project root.
pub const FILE_NAME: &str = "jackdaw.toml";

/// The versions a project was set up against. Written once, then
/// compared on every open so a jackdaw or Bevy upgrade is reported
/// rather than discovered as a confusing build failure.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionPins {
    /// The full jackdaw version that set the project up, e.g. `0.19.0`.
    #[serde(default)]
    pub version: Option<String>,
    /// The Bevy minor that jackdaw targeted then, e.g. `0.19`.
    #[serde(default)]
    pub bevy: Option<String>,
}

/// The subset of `jackdaw.toml` the build pipeline reads.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct ProjectManifest {
    /// The game plugin type inside the project's lib crate.
    #[serde(default)]
    pub plugin: Option<String>,
    /// Which workspace member is the game. Absent for single-package
    /// projects, where resolution is unambiguous.
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub jackdaw: VersionPins,
}

impl ProjectManifest {
    /// Parse `<root>/jackdaw.toml`. Missing or unreadable yields the
    /// defaults, so callers treat "not set up yet" and "set up with no
    /// overrides" the same way.
    ///
    /// A file that exists but does not parse also yields the defaults,
    /// which silently discards the user's `plugin`, `package`, and
    /// pins. Callers that can report to the user should use
    /// [`ProjectManifest::read_checked`] so the parse error is not
    /// swallowed.
    pub fn read(root: &Path) -> Self {
        Self::read_checked(root).unwrap_or_default()
    }

    /// As [`ProjectManifest::read`], surfacing why an existing file
    /// could not be parsed. A missing file is `Ok(default)`: not being
    /// set up is not an error.
    pub fn read_checked(root: &Path) -> Result<Self, String> {
        let Ok(text) = std::fs::read_to_string(path(root)) else {
            return Ok(Self::default());
        };
        toml::from_str(&text).map_err(|error| error.to_string())
    }

    /// Whether this project has been set up for jackdaw at all.
    pub fn exists(root: &Path) -> bool {
        path(root).is_file()
    }
}

pub fn path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

/// How a project's pins compare to the running jackdaw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinStatus {
    /// No pins recorded (set up by an older jackdaw, or hand-written).
    Unpinned,
    Match,
    /// Same Bevy minor, different jackdaw patch or minor. Safe to open;
    /// worth reporting because the project's own dependency requirements
    /// may want bumping.
    JackdawDiffers {
        pinned: String,
        running: String,
    },
    /// A different Bevy minor: the project's crates and the editor's SDK
    /// cannot share a type graph, so this needs the user's attention.
    BevyDiffers {
        pinned: String,
        running: String,
    },
}

impl PinStatus {
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::BevyDiffers { .. })
    }
}

/// Compare a project's recorded pins to this build's versions.
///
/// Prefer [`compare_project`] where a project root is available: pins
/// are absent from anything set up before they existed or written by
/// hand, and an absent pin must not read as "compatible".
pub fn compare_pins(pins: &VersionPins) -> PinStatus {
    if let Some(pinned) = &pins.bevy
        && pinned != crate::BEVY_VERSION
    {
        return PinStatus::BevyDiffers {
            pinned: pinned.clone(),
            running: crate::BEVY_VERSION.to_string(),
        };
    }
    match &pins.version {
        None => PinStatus::Unpinned,
        Some(pinned) if pinned == crate::VERSION => PinStatus::Match,
        Some(pinned) => PinStatus::JackdawDiffers {
            pinned: pinned.clone(),
            running: crate::VERSION.to_string(),
        },
    }
}

/// Compare a project against this build, falling back to the Bevy
/// requirement its own manifest declares when `jackdaw.toml` records no
/// pins.
///
/// Without the fallback an unpinned project (set up by a jackdaw that
/// predates pins, or a hand-written `jackdaw.toml`) opens with no
/// warning and then fails to build with a wall of Bevy type errors,
/// which is exactly what pinning exists to prevent.
pub fn compare_project(root: &Path, manifest: &ProjectManifest) -> PinStatus {
    let status = compare_pins(&manifest.jackdaw);
    if !matches!(status, PinStatus::Unpinned) || manifest.jackdaw.bevy.is_some() {
        return status;
    }
    match declared_bevy_minor(root) {
        Some(minor) if minor != crate::BEVY_VERSION => PinStatus::BevyDiffers {
            pinned: minor,
            running: crate::BEVY_VERSION.to_string(),
        },
        _ => status,
    }
}

/// The Bevy minor a project targets: its recorded pin, else the
/// requirement its own manifest declares. `None` when neither states
/// one. Cheap enough for a launcher list to call per row.
pub fn targeted_bevy(root: &Path) -> Option<String> {
    ProjectManifest::read(root)
        .jackdaw
        .bevy
        .or_else(|| declared_bevy_minor(root))
}

/// The Bevy minor the project's own Cargo manifest declares, ignoring
/// any recorded pin. What an upgrade needs: the pin describes what the
/// project targets, so refreshing it must follow the manifest rather
/// than the other way round.
pub fn targeted_bevy_from_manifest(root: &Path) -> Option<String> {
    declared_bevy_minor(root)
}

/// The `major.minor` of the bevy requirement in `<root>/Cargo.toml`, for
/// projects that record no pins. Deliberately shallow: only a plain
/// `x.y`-shaped requirement is conclusive enough to block an open on.
fn declared_bevy_minor(root: &Path) -> Option<String> {
    // In a workspace the game is a member, and its bevy requirement is
    // commonly inherited from the root. Reading only `<root>/Cargo.toml`
    // finds nothing for either shape, which silently turns the whole
    // check (and the launcher's version badge) into a no-op.
    let package_dir = crate::cargo_meta::resolve_project_package(
        root,
        ProjectManifest::read(root).package.as_deref(),
    )
    .map(|package| package.dir)
    .unwrap_or_else(|_| root.to_path_buf());

    let member: toml::Value =
        toml::from_str(&std::fs::read_to_string(package_dir.join("Cargo.toml")).ok()?).ok()?;
    let bevy = member.get("dependencies")?.get("bevy")?;
    let inherits = bevy
        .get("workspace")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let owned;
    let bevy = if inherits {
        let workspace: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).ok()?).ok()?;
        owned = workspace
            .get("workspace")?
            .get("dependencies")?
            .get("bevy")?
            .clone();
        &owned
    } else {
        bevy
    };
    let req = bevy
        .as_str()
        .or_else(|| bevy.get("version")?.as_str())?
        .trim_start_matches(['^', '=', '~']);
    let mut parts = req.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let conclusive = major.chars().all(|c| c.is_ascii_digit())
        && minor.chars().all(|c| c.is_ascii_digit())
        && !major.is_empty()
        && !minor.is_empty();
    conclusive.then(|| format!("{major}.{minor}"))
}

/// The `[jackdaw]` block written into a new or imported project.
pub fn pins_toml() -> String {
    pins_toml_for_bevy(crate::BEVY_VERSION)
}

/// As [`pins_toml`], recording a Bevy minor other than this build's.
///
/// Used when a project is set up despite targeting a different Bevy:
/// the pin has to describe the project, so the mismatch keeps being
/// reported until it is actually resolved.
pub fn pins_toml_for_bevy(bevy: &str) -> String {
    format!(
        "# Recorded when this project was set up. Jackdaw compares these on\n\
         # open so an editor upgrade is reported instead of surfacing as a\n\
         # build failure.\n\
         [jackdaw]\n\
         version = \"{}\"\n\
         bevy = \"{bevy}\"\n",
        crate::VERSION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_manifest_reads_as_default() {
        let manifest = ProjectManifest::read(Path::new("/definitely/not/here"));
        assert!(manifest.plugin.is_none());
        assert!(manifest.package.is_none());
        assert_eq!(manifest.jackdaw, VersionPins::default());
    }

    #[test]
    fn build_keys_parse_alongside_run_configs() {
        let manifest: ProjectManifest = toml::from_str(
            r#"
plugin = "MyPlugin"
package = "game"

[jackdaw]
version = "0.19.0"
bevy = "0.19"

[[run]]
name = "Play"
instances = 2
"#,
        )
        .unwrap();
        assert_eq!(manifest.plugin.as_deref(), Some("MyPlugin"));
        assert_eq!(manifest.package.as_deref(), Some("game"));
        assert_eq!(manifest.jackdaw.version.as_deref(), Some("0.19.0"));
        assert_eq!(manifest.jackdaw.bevy.as_deref(), Some("0.19"));
    }

    #[test]
    fn a_manifest_without_pins_is_unpinned() {
        assert_eq!(compare_pins(&VersionPins::default()), PinStatus::Unpinned);
    }

    #[test]
    fn current_pins_match() {
        let pins = VersionPins {
            version: Some(crate::VERSION.to_string()),
            bevy: Some(crate::BEVY_VERSION.to_string()),
        };
        assert_eq!(compare_pins(&pins), PinStatus::Match);
    }

    #[test]
    fn a_bevy_minor_change_blocks_and_a_jackdaw_patch_change_does_not() {
        let bevy = VersionPins {
            version: Some(crate::VERSION.to_string()),
            bevy: Some("0.14".to_string()),
        };
        assert!(compare_pins(&bevy).is_blocking());

        let jackdaw = VersionPins {
            version: Some("0.0.1".to_string()),
            bevy: Some(crate::BEVY_VERSION.to_string()),
        };
        let status = compare_pins(&jackdaw);
        assert!(!status.is_blocking());
        assert!(matches!(status, PinStatus::JackdawDiffers { .. }));
    }

    fn project(name: &str, cargo_toml: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jackdaw_pins_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), cargo_toml).unwrap();
        dir
    }

    /// A project set up before pins existed must still be caught, or the
    /// user only learns about the mismatch from a wall of type errors.
    #[test]
    fn an_unpinned_project_falls_back_to_its_own_bevy_requirement() {
        let dir = project(
            "old",
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"0.14\"\n",
        );
        let status = compare_project(&dir, &ProjectManifest::default());
        assert!(status.is_blocking(), "got {status:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unpinned_project_on_the_supported_bevy_is_only_unpinned() {
        let dir = project(
            "current",
            &format!(
                "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n[dependencies]\nbevy = \"{}\"\n",
                crate::BEVY_VERSION
            ),
        );
        assert_eq!(
            compare_project(&dir, &ProjectManifest::default()),
            PinStatus::Unpinned
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A git or path bevy dependency states no version, so there is
    /// nothing to compare and the open must not be blocked on a guess.
    #[test]
    fn an_unpinned_project_with_no_stated_bevy_version_is_not_blocked() {
        let dir = project(
            "git",
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n\
             [dependencies]\nbevy = { git = \"https://github.com/bevyengine/bevy\" }\n",
        );
        assert_eq!(
            compare_project(&dir, &ProjectManifest::default()),
            PinStatus::Unpinned
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_pins_round_trip_to_a_match() {
        let manifest: ProjectManifest = toml::from_str(&pins_toml()).unwrap();
        assert_eq!(compare_pins(&manifest.jackdaw), PinStatus::Match);
    }
}
