use std::path::PathBuf;

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("jackdaw"))
}

pub fn recent_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent.json"))
}

pub fn last_new_project_location_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("last_new_project_location"))
}

pub fn keybinds_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("keybinds.json"))
}
