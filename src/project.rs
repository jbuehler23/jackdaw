use std::{
    fs,
    path::{Path, PathBuf},
};

use bevy::prelude::*;
use jackdaw_api_internal::paths::{last_new_project_location_path, recent_file_path};
use jackdaw_jsn::format::{JsnHeader, JsnProject, JsnProjectConfig};
use serde::{Deserialize, Serialize};

/// Resource holding the active project root directory and its config.
#[derive(Resource)]
pub struct ProjectRoot {
    pub root: PathBuf,
    pub config: JsnProject,
}

impl ProjectRoot {
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
        let path = path.as_ref();
        path.strip_prefix(&self.root).unwrap_or(path).into()
    }
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

pub fn read_last_project() -> Option<PathBuf> {
    let recent = read_recent_projects();
    recent.projects.first().map(|e| e.path.clone())
}

pub fn save_project_config(root: &Path, project: &JsnProject) -> std::io::Result<()> {
    let dir = root.join(".jackdaw");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("project.json");
    let data = serde_json::to_string_pretty(project)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, data)
}

pub fn load_project_config(root: &Path) -> Option<JsnProject> {
    // Current location: local editor state under the gitignored `.jackdaw/`.
    let new_path = root.join(".jackdaw").join("project.json");
    if new_path.is_file() {
        return std::fs::read_to_string(&new_path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok());
    }
    // Legacy locations (newest first). Migrate on read so the next save
    // writes to `.jackdaw/project.json` and the old file falls out of use.
    for legacy in [root.join(".jsn").join("project.jsn"), root.join("project.jsn")] {
        if legacy.is_file()
            && let Ok(data) = std::fs::read_to_string(&legacy)
            && let Ok(project) = serde_json::from_str::<JsnProject>(&data)
        {
            warn!(
                "Migrating project config {} -> .jackdaw/project.json",
                legacy.display()
            );
            let _ = save_project_config(root, &project);
            return Some(project);
        }
    }
    None
}

pub fn create_default_project(root: &Path) -> JsnProject {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled Project".to_string());

    let project = JsnProject {
        jsn: JsnHeader::default(),
        project: JsnProjectConfig {
            name,
            description: String::new(),
            default_scene: None,
            layout: None,
            ..Default::default()
        },
    };

    // Write local editor state under the gitignored `.jackdaw/`.
    let dir = root.join(".jackdaw");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("project.json");
    if let Ok(data) = serde_json::to_string_pretty(&project) {
        let _ = std::fs::write(&path, data);
    }

    project
}

/// Remove a project from the recent projects list.
pub fn remove_recent(path: &Path) {
    let mut recent = read_recent_projects();
    recent.projects.retain(|e| e.path != path);
    save_recent_projects(&recent);
}

/// Record a project in the recent projects list.
pub fn touch_recent(root: &Path, name: &str) {
    let mut recent = read_recent_projects();

    // Remove existing entry for this path
    recent.projects.retain(|e| e.path != root);

    // Insert at the front
    recent.projects.insert(
        0,
        RecentEntry {
            path: root.to_path_buf(),
            name: name.to_string(),
            last_opened: crate::timestamps::utc_rfc3339_now(),
        },
    );

    // Keep at most 10
    recent.projects.truncate(10);

    save_recent_projects(&recent);
}
