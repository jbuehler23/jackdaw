//! Disk persistence for the active keymap preset.

use std::path::PathBuf;

use bevy::prelude::*;

use super::types::ActiveKeymapPreset;

fn keymap_preset_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("keymap_preset.json"))
}

/// Load the active keymap preset from disk. Returns the default ("classic")
/// silently if the file is absent, or with a `warn!` if the file is present
/// but cannot be parsed.
pub fn load_active_keymap_preset() -> ActiveKeymapPreset {
    let Some(path) = keymap_preset_path() else {
        return ActiveKeymapPreset::default();
    };
    if !path.is_file() {
        return ActiveKeymapPreset::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read keymap preset file {}: {e}", path.display());
            return ActiveKeymapPreset::default();
        }
    };
    match serde_json::from_str::<ActiveKeymapPreset>(&data) {
        Ok(preset) => preset,
        Err(e) => {
            warn!(
                "Corrupt keymap preset file {}; falling back to default: {e}",
                path.display()
            );
            ActiveKeymapPreset::default()
        }
    }
}

/// Persist the active keymap preset to disk.
pub fn save_active_keymap_preset(preset: &ActiveKeymapPreset) {
    let Some(path) = keymap_preset_path() else {
        warn!("Could not determine config directory for keymap preset");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(preset) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to write keymap preset file: {e}");
            }
        }
        Err(e) => {
            warn!("Failed to serialize keymap preset: {e}");
        }
    }
}
