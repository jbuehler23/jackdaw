use std::path::PathBuf;

pub const DATA_DIR_NAME: &str = "jackdaw";
pub const DATA_DIR_FALLBACK_NAME: &str = ".jackdaw";

pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join(DATA_DIR_NAME))
        .or_else(data_dir_fallback)
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir()
        .map(|d| d.join(DATA_DIR_NAME))
        .or_else(data_dir_fallback)
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

fn data_dir_fallback() -> Option<PathBuf> {
    std::env::home_dir().map(|p| p.join(DATA_DIR_FALLBACK_NAME))
}
