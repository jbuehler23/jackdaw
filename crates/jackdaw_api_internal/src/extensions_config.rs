//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. Read on startup, rewritten
//! whenever the user toggles an extension.

use std::{collections::BTreeMap, path::PathBuf};

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_platform::collections::HashSet;
use serde::{Deserialize, Serialize};

use crate::paths::config_dir;

/// On-disk shape. Maps extension IDs to their configuration.
#[derive(Serialize, Deserialize, Default, Deref, DerefMut)]
pub struct ExtensionsConfig(BTreeMap<String, ExtensionConfig>);

#[derive(Serialize, Deserialize)]
pub struct ExtensionConfig {
    /// Whether the extension is enabled.
    pub enabled: bool,
}

impl Default for ExtensionConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("extensions.json"))
}

// TODO: This should all be an `Asset` instead of raw file access

/// Read the enabled list from disk. Returns `None` if the file doesn't
/// exist; callers should interpret that as "enable everything".
pub fn read_extension_config() -> Option<ExtensionsConfig> {
    let path = config_path()?;
    // TODO: if only a single extension is malformed, we should probably not throw away the whole list.
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Add the extension with the given ID to the enabled list if it is not already present.
// TODO: this will always enable it, since the enabled list doesn't list disabled extensions
pub fn init_extension(id: impl Into<String>) {
    let id = id.into();
    let mut config = read_extension_config().unwrap_or_default();
    config.entry(id).or_default().enabled = true;
    write_enabled_list(&config);
}

/// Write the currently-enabled list to disk.
pub fn write_enabled_list(config: &ExtensionsConfig) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&path, data);
    }
}

/// Compute the current enabled list from the loaded `Extension` entities
/// and write it to disk.
pub fn persist_current_enabled(world: &mut World) {
    let mut query = world.query::<&crate::lifecycle::Extension>();
    let enabled: HashSet<String> = query.iter(world).map(|e| e.id.clone()).collect();
    let mut config = read_extension_config().unwrap_or_default();
    for (id, ext_config) in config.iter_mut() {
        if !enabled.contains(id) {
            ext_config.enabled = false;
        }
    }
    for id in enabled {
        config.entry(id).or_default().enabled = true;
    }

    write_enabled_list(&config);
}
