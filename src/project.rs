use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_jsn::format::{JsnHeader, JsnProject, JsnProjectConfig};
use serde::{Deserialize, Serialize};

/// Resource holding the active project root directory and its config.
#[derive(Resource)]
pub struct ProjectRoot {
    pub root: PathBuf,
    pub config: JsnProject,
}

impl ProjectRoot {
    pub fn jsn_dir(&self) -> PathBuf {
        self.root.join(".jsn")
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

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("jackdaw"))
}

fn recent_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent.json"))
}

pub fn read_recent_projects() -> RecentProjects {
    let Some(path) = recent_file_path() else {
        return RecentProjects::default();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return RecentProjects::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
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

pub fn read_last_project() -> Option<PathBuf> {
    let recent = read_recent_projects();
    recent.projects.first().map(|e| e.path.clone())
}

pub fn load_project_config(root: &Path) -> Option<JsnProject> {
    // Prefer .jsn/ directory, fall back to legacy root location
    let new_path = root.join(".jsn/project.jsn");
    let legacy_path = root.join("project.jsn");
    let path = if new_path.is_file() {
        new_path
    } else {
        legacy_path
    };
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
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
        },
    };

    // Write to .jsn/ directory
    let jsn_dir = root.join(".jsn");
    let _ = std::fs::create_dir_all(&jsn_dir);
    let path = jsn_dir.join("project.jsn");
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
            last_opened: chrono_now(),
        },
    );

    // Keep at most 10
    recent.projects.truncate(10);

    save_recent_projects(&recent);
}

fn chrono_now() -> String {
    // Simple ISO 8601-ish timestamp without pulling in chrono
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Just store the unix timestamp, good enough for sorting.
    format!("{secs}")
}
