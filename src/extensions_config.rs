//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. Read on startup, rewritten
//! whenever the user toggles an extension.

use std::collections::HashSet;
use std::path::PathBuf;

use bevy::{platform::collections::HashMap, prelude::*};
use jackdaw_api::prelude::ExtensionKind;
use jackdaw_api_internal::lifecycle::ExtensionCatalog;
use serde::{Deserialize, Serialize};

/// Extensions that must always be loaded — the editor panics without
/// the resources they install. Anything listed here is force-enabled
/// in [`resolve_enabled_list`] regardless of what's persisted on
/// disk, so a stale config (e.g. one written before the extension
/// was extracted) can't take the editor down. The Extensions dialog
/// should also hide or lock these so users can't try to turn them
/// off.
pub const REQUIRED_EXTENSIONS: &[&str] = &[crate::core_extension::CORE_EXTENSION_ID];

/// True if the named extension is load-bearing and must not be
/// user-toggleable.
pub fn is_required(name: &str) -> bool {
    REQUIRED_EXTENSIONS.contains(&name)
}

/// On-disk shape.
#[derive(Serialize, Deserialize, Default)]
pub struct ExtensionsConfig {
    pub enabled: Vec<String>,
}

fn config_path() -> Option<PathBuf> {
    crate::project::config_dir().map(|d| d.join("extensions.json"))
}

/// Read the enabled list from disk. Returns `None` if the file doesn't
/// exist; callers should interpret that as "enable everything".
pub fn read_enabled_list() -> Option<Vec<String>> {
    let path = config_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let config: ExtensionsConfig = serde_json::from_str(&data).ok()?;
    Some(config.enabled)
}

/// Write the currently-enabled list to disk.
pub fn write_enabled_list(enabled: &[String]) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let config = ExtensionsConfig {
        enabled: enabled.to_vec(),
    };
    if let Ok(data) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&path, data);
    }
}

/// Resolve which catalog entries to enable on startup.
///
/// Pre-dogfood files list none of the built-ins; fall back to enabling
/// everything so the editor stays usable until the next toggle rewrites
/// the file. Files that already record at least one built-in are
/// trusted exactly as written.
pub fn resolve_enabled_list(world: &World) -> Vec<String> {
    let catalog = world.resource::<ExtensionCatalog>();
    let available: Vec<String> = catalog.iter().map(ToString::to_string).collect();
    let builtins: HashMap<String, String> = catalog
        .iter_with_content()
        .filter(|(.., kind)| *kind == ExtensionKind::Builtin)
        .map(|(id, label, ..)| (id.to_string(), label.to_string()))
        .collect();

    let mut resolved = match read_enabled_list() {
        Some(list) => {
            let on_disk: HashSet<String> = list.into_iter().collect();
            let has_any_builtin = builtins.keys().any(|id| on_disk.contains(id));
            if !has_any_builtin {
                available.clone()
            } else {
                available
                    .iter()
                    .filter(|n| on_disk.contains(*n))
                    .cloned()
                    .collect()
            }
        }
        None => available.clone(),
    };

    // Force-include any REQUIRED extension the catalog knows about
    // but the resolved list dropped (e.g. because the persisted
    // config predates it). Without this, upgrading into a build that
    // extracted a resource into a new required extension panics on
    // first launch.
    for required in REQUIRED_EXTENSIONS {
        let in_catalog = available.iter().any(|n| n == required);
        let already_listed = resolved.iter().any(|n| n == required);
        if in_catalog && !already_listed {
            resolved.push((*required).to_string());
        }
    }

    resolved
}

/// Compute the current enabled list from the loaded `Extension` entities
/// and write it to disk.
pub fn persist_current_enabled(world: &mut World) {
    let mut query = world.query::<&jackdaw_api_internal::lifecycle::Extension>();
    let enabled: Vec<String> = query.iter(world).map(|e| e.id.clone()).collect();
    write_enabled_list(&enabled);
}
