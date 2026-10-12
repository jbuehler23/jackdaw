//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. Read on startup, rewritten
//! whenever the user toggles an extension.

use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::paths::config_dir;

/// On-disk shape. Maps extension IDs to their configuration.
#[derive(Serialize, Deserialize, Default)]
pub struct ExtensionsConfig(BTreeMap<String, ExtensionConfig>);

impl Deref for ExtensionsConfig {
    type Target = BTreeMap<String, ExtensionConfig>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ExtensionsConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// One extension's persisted settings.
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
pub fn init_extension(id: impl Into<String>) {
    let id = id.into();
    let mut config = read_extension_config().unwrap_or_default();
    if config.contains_key(&id) {
        return;
    }
    config.insert(id, ExtensionConfig::default());
    write_enabled_list(&config);
}

/// Whether the user has left the extension on. An extension the file does not
/// name counts as on, as it does at startup.
pub fn is_extension_enabled(id: &str) -> bool {
    read_extension_config()
        .and_then(|config| config.get(id).map(|entry| entry.enabled))
        .unwrap_or(true)
}

/// Persist one extension's desired state without requiring a running editor.
pub fn set_extension_enabled(id: impl Into<String>, enabled: bool) {
    let mut config = read_extension_config().unwrap_or_default();
    config.entry(id.into()).or_default().enabled = enabled;
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
