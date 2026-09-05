use std::{
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use jackdaw_env::paths::{last_new_project_location_path, recent_file_path};
use serde::{Deserialize, Serialize};

/// Resource holding the active project root directory and its config.
#[derive(Resource)]
pub struct ProjectRoot {
    pub root: PathBuf,
    pub config: ProjectConfig,
}

/// The open project's `assets/` directory, mirrored out of the ECS.
///
/// [`ProjectRoot`] is the answer, but the path helpers that need it are
/// plain functions called from observers, asset loaders and scene
/// readers that hold no `World`. Without somewhere to read it they fell
/// back to the recents file, which meant opening and parsing
/// `recent.json` once per call -- hundreds of times a frame on a scene
/// with hundreds of models.
///
/// One process opens one project, so a static mirror says exactly what
/// the resource does. `None` before a project is open, which is the only
/// case that still has to go to disk.
static OPEN_PROJECT_ASSETS: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// The open project's assets directory, or `None` when no project is
/// open or its `assets/` does not exist.
pub fn open_project_assets_dir() -> Option<PathBuf> {
    OPEN_PROJECT_ASSETS.read().ok()?.clone()
}

/// Point the mirror at `dir`, or clear it. Only `mirror_open_project`
/// should call this; it is public for tests that need the helpers to
/// behave as though a project were open.
pub fn set_open_project_assets_dir(dir: Option<PathBuf>) {
    if let Ok(mut slot) = OPEN_PROJECT_ASSETS.write() {
        *slot = dir;
    }
}

/// Keep [`open_project_assets_dir`] in step with the resource.
///
/// A whole system for one comparison, rather than a write at each place
/// that inserts [`ProjectRoot`], because a missed write there is a
/// silent wrong answer everywhere the helpers are used.
pub(crate) fn mirror_open_project(
    project: Option<Res<ProjectRoot>>,
    mut mirrored: Local<Option<PathBuf>>,
) {
    // Validated here, once per change, so the hot path is a lock read.
    let current = project
        .map(|project| project.assets_dir())
        .filter(|assets| assets.is_dir());
    if *mirrored == current {
        return;
    }
    set_open_project_assets_dir(current.clone());
    *mirrored = current;
}

/// Native editor project configuration persisted to `.jackdaw/project.json`.
/// Carries the same fields the legacy JSN project config held, without the
/// vestigial format-header wrapper.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ProjectConfig {
    /// Human-readable project name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Default scene to open, relative to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scene: Option<String>,
    /// Persisted editor layout state. Opaque here; consumers parse it as
    /// the `jackdaw_panels` workspace state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<serde_json::Value>,
    /// Scene paths (relative to the project root) open in the tab strip when
    /// the project was last closed. Restored in order on the next launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_open_tabs: Vec<String>,
    /// Index into `last_open_tabs` of the tab that was active. Clamped on load.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_active_tab: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Legacy on-disk shape: `.jackdaw/project.json` and `project.jsn` files
/// written before the native config, which wrapped the fields as
/// `{"jsn":{..},"project":{..}}`.
#[derive(Deserialize)]
struct LegacyNestedProject {
    project: ProjectConfig,
}

/// Parse a project config from either the new flat shape or the legacy
/// nested `{"jsn":..,"project":..}` wrapper. Flat is tried first; the nested
/// form has no top-level `name`, so it only reaches the compat fallback.
fn parse_project_config(data: &str) -> Option<ProjectConfig> {
    if let Ok(config) = serde_json::from_str::<ProjectConfig>(data) {
        return Some(config);
    }
    serde_json::from_str::<LegacyNestedProject>(data)
        .ok()
        .map(|nested| nested.project)
}

impl ProjectRoot {
    pub fn new(root: impl Into<PathBuf>, config: ProjectConfig) -> Self {
        let root = root.into();
        Self {
            root: dunce::simplified(&root).to_path_buf(),
            config,
        }
    }

    /// The `.jackdaw/` directory: gitignored, regenerated build artifacts
    /// (schema, plan, shim, target) plus local editor state
    /// (`project.json`) and caches (`registry.json`). Committed project data
    /// (the manifest, scenes, the asset catalog) lives outside it.
    pub fn jackdaw_dir(&self) -> PathBuf {
        self.root.join(".jackdaw")
    }
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn to_relative(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = dunce::simplified(path.as_ref());
        let root = dunce::simplified(&self.root);
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    }
}

/// Resolve `candidate` under `root`, refusing anything that would land
/// outside it.
///
/// The editor runs as the user, so a path taken verbatim from a remote
/// caller or an operator parameter is an arbitrary write:
/// `../.ssh/config` names the user's key config as readily as it names a
/// scene. A path the project owns is relative, carries no `..`, and lands
/// under `root` once resolved.
///
/// The file itself need not exist yet -- a write is usually about to
/// create it -- so the deepest existing path on the way to it is what
/// gets canonicalized, which is where a symlink would otherwise smuggle
/// the write back out. The target is included when it exists: a write
/// follows a symlink in the last component as readily as one in a
/// directory above it.
pub fn path_within(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    if candidate.is_absolute() {
        return Err(format!(
            "`{}` is absolute; name a path relative to {}",
            candidate.display(),
            root.display()
        ));
    }
    if candidate
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!(
            "`{}` climbs out of {}",
            candidate.display(),
            root.display()
        ));
    }
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let resolved = root.join(candidate);
    let anchor = resolved
        .ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(&root);
    let anchor = dunce::canonicalize(anchor).unwrap_or_else(|_| anchor.to_path_buf());
    if !anchor.starts_with(&root) {
        return Err(format!(
            "`{}` resolves outside {}",
            candidate.display(),
            root.display()
        ));
    }
    Ok(resolved)
}

#[derive(Serialize, Deserialize, Default)]
pub struct RecentProjects {
    pub projects: Vec<RecentEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RecentEntry {
    pub path: PathBuf,
    pub name: String,
    pub last_opened: String,
}

pub fn read_recent_projects() -> RecentProjects {
    let Some(path) = recent_file_path() else {
        return RecentProjects::default();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return RecentProjects::default();
    };

    let mut projects: RecentProjects = serde_json::from_str(&data).unwrap_or_default();

    projects
        .projects
        .retain(|entry| fs::exists(entry.path.clone()).unwrap_or_default());

    projects
}

pub fn save_recent_projects(projects: &RecentProjects) {
    let Some(path) = recent_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(projects) {
        let _ = std::fs::write(&path, data);
    }
}

/// Remembered parent directory used the last time the user created
/// a project from the launcher. `None` if never set or unreadable.
pub fn read_last_new_project_location() -> Option<PathBuf> {
    let path = last_new_project_location_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

pub fn save_last_new_project_location(location: &Path) {
    let Some(path) = last_new_project_location_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, location.to_string_lossy().as_bytes());
}

/// Environment variable naming the project the editor should open,
/// set by `jd open <path>` on the process it spawns.
pub const ENV_OPEN_PROJECT: &str = "JACKDAW_OPEN_PROJECT";

/// The project a `jd open` invocation asked this process to open.
pub fn requested_project() -> Option<PathBuf> {
    std::env::var_os(ENV_OPEN_PROJECT)
        .map(PathBuf::from)
        .map(|path| dunce::simplified(&path).to_path_buf())
        .filter(|path| path.is_dir())
}

pub fn read_last_project() -> Option<PathBuf> {
    let recent = read_recent_projects();
    recent
        .projects
        .first()
        .map(|entry| dunce::simplified(entry.path.as_path()).to_path_buf())
}

pub fn save_project_config(root: &Path, config: &ProjectConfig) -> std::io::Result<()> {
    let dir = root.join(".jackdaw");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("project.json");
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, data)
}

pub fn load_project_config(root: &Path) -> Option<ProjectConfig> {
    // Current location: local editor state under the gitignored `.jackdaw/`.
    let new_path = root.join(".jackdaw").join("project.json");
    if new_path.is_file() {
        return std::fs::read_to_string(&new_path)
            .ok()
            .and_then(|data| parse_project_config(&data));
    }
    // Legacy locations (newest first). Migrate on read so the next save
    // writes to `.jackdaw/project.json` and the old file falls out of use.
    for legacy in [
        root.join(".jsn").join("project.jsn"),
        root.join("project.jsn"),
    ] {
        if legacy.is_file()
            && let Ok(data) = std::fs::read_to_string(&legacy)
            && let Some(config) = parse_project_config(&data)
        {
            warn!(
                "Migrating project config {} -> .jackdaw/project.json",
                legacy.display()
            );
            let _ = save_project_config(root, &config);
            return Some(config);
        }
    }
    None
}

pub fn create_default_project(root: &Path) -> ProjectConfig {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled Project".to_string());

    let config = ProjectConfig {
        name,
        ..Default::default()
    };

    // Write local editor state under the gitignored `.jackdaw/`.
    let dir = root.join(".jackdaw");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("project.json");
    if let Ok(data) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&path, data);
    }

    config
}

/// Remove a project from the recent projects list.
pub fn remove_recent(path: &Path) {
    let path = dunce::simplified(path);
    let mut recent = read_recent_projects();
    recent
        .projects
        .retain(|entry| dunce::simplified(entry.path.as_path()) != path);
    save_recent_projects(&recent);
}

/// Record a project in the recent projects list.
pub fn touch_recent(root: &Path, name: &str) {
    let root = dunce::simplified(root).to_path_buf();
    let mut recent = read_recent_projects();

    // Remove existing entry for this path
    recent
        .projects
        .retain(|entry| dunce::simplified(entry.path.as_path()) != root);

    // Insert at the front
    recent.projects.insert(
        0,
        RecentEntry {
            path: root,
            name: name.to_string(),
            last_opened: crate::timestamps::utc_rfc3339_now(),
        },
    );

    // Keep at most 10
    recent.projects.truncate(10);

    save_recent_projects(&recent);
}
