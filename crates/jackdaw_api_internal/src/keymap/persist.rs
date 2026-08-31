//! Disk persistence for the active keymap preset and the user's own
//! per-operator bindings.

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::types::{ActiveKeymapPreset, PresetBinding};

fn keymap_preset_path() -> Option<PathBuf> {
    jackdaw_env::paths::config_dir().map(|d| d.join("keymap_preset.json"))
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

/// The user's own bindings, layered over the shipped defaults by
/// [`super::resolve_keymap`]. Only operators the user has actually
/// rebound appear here: an operator with no row keeps every default
/// row it was registered with, which is what makes a "reset" a
/// deletion rather than a copy of the defaults.
///
/// Persisted as JSON at [`jackdaw_env::paths::keymap_path`]. The file
/// is written only from the settings dialog's own diff, never from an
/// applied keymap, so an entry naming an operator that is not loaded
/// right now survives the sessions that cannot resolve it.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserKeymap {
    #[serde(default)]
    pub bindings: Vec<PresetBinding>,
}

/// Read the user keymap from disk. An absent file is an empty keymap;
/// an unreadable or corrupt one warns and is treated as empty, so a bad
/// file costs the user their overrides and not the editor's input.
pub fn load_user_keymap() -> UserKeymap {
    let Some(path) = jackdaw_env::paths::keymap_path() else {
        return UserKeymap::default();
    };
    if !path.is_file() {
        return UserKeymap::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read keymap file {}: {e}", path.display());
            return UserKeymap::default();
        }
    };
    match serde_json::from_str::<UserKeymap>(&data) {
        Ok(keymap) => keymap,
        Err(e) => {
            warn!(
                "Corrupt keymap file {}; ignoring user bindings: {e}",
                path.display()
            );
            UserKeymap::default()
        }
    }
}

/// Write the user keymap to disk.
pub fn save_user_keymap(keymap: &UserKeymap) {
    let Some(path) = jackdaw_env::paths::keymap_path() else {
        warn!("Could not determine config directory for the keymap file");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(keymap) {
        Ok(data) => {
            if let Err(e) = std::fs::write(&path, data) {
                warn!("Failed to write keymap file: {e}");
            }
        }
        Err(e) => warn!("Failed to serialize the keymap: {e}"),
    }
}
