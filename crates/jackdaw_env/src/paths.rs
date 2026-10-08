use std::path::PathBuf;

pub const DATA_DIR_NAME: &str = "jackdaw";
pub const DATA_DIR_FALLBACK_NAME: &str = ".jackdaw";

pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir()
        .map(|p| p.join(DATA_DIR_NAME))
        .or_else(data_dir_fallback)
}

/// Where regenerable per-user data lives, such as the SDK build cache.
///
/// The same directory as [`data_dir`] on Linux and macOS. On Windows it is
/// the local rather than the roaming application data, so gigabytes of build
/// output are not synced with a roaming profile.
pub fn local_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir()
        .map(|p| p.join(DATA_DIR_NAME))
        .or_else(data_dir_fallback)
}

/// Environment variable naming the directory the editor keeps its own
/// configuration in.
///
/// Set, it replaces the per-user config directory whole: the keymap, the
/// keybinds, the recent-project list and the extension list all move with it.
/// A test sets it so the run reads its own fixture rather than the config of
/// whoever is running it, and a session can be pointed at a scratch directory
/// the same way.
pub const CONFIG_DIR_VAR: &str = "JACKDAW_CONFIG_DIR";

pub fn config_dir() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os(CONFIG_DIR_VAR).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(overridden));
    }
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

/// Where the user's per-operator keymap overrides live.
pub fn keymap_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("keymap.json"))
}

/// Where the user's viewport quality and display settings live.
pub fn viewport_settings_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("viewport.json"))
}

/// Where state the editor keeps between runs but the user does not edit
/// lives: the extension quarantine, and anything else of that kind.
///
/// Named here rather than reached for with `dirs` at each use, so the
/// directory is decided in one place -- and so a session pointed at a scratch
/// config directory keeps its quarantine there too, rather than writing into
/// the real one.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os(CONFIG_DIR_VAR).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(overridden).join("state"));
    }
    dirs::state_dir()
        .map(|d| d.join(DATA_DIR_NAME))
        .or_else(data_dir_fallback)
}

fn data_dir_fallback() -> Option<PathBuf> {
    std::env::home_dir().map(|p| p.join(DATA_DIR_FALLBACK_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regenerable_data_stays_off_the_roaming_profile() {
        let local = local_data_dir().expect("a local data directory");
        if cfg!(windows) {
            let roaming = data_dir().expect("a data directory");
            assert_ne!(local, roaming);
            let appdata = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set");
            assert_eq!(local, PathBuf::from(appdata).join(DATA_DIR_NAME));
        } else {
            assert_eq!(Some(local), data_dir());
        }
    }
}
