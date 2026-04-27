//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. Read on startup, rewritten
//! whenever the user toggles an extension.

use bevy::{platform::collections::HashMap, prelude::*};
use jackdaw_api::prelude::ExtensionKind;
use jackdaw_api_internal::{extensions_config::read_extension_config, lifecycle::ExtensionCatalog};

/// Extensions that must always be loaded; the editor panics without
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

    let mut resolved = match read_extension_config() {
        Some(config) => {
            let has_any_builtin = builtins.keys().any(|id| config.contains_key(id));
            if !has_any_builtin {
                available.clone()
            } else {
                available
                    .iter()
                    .filter(|n| config.contains_key(*n))
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
