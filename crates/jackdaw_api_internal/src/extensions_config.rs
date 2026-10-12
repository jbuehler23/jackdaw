//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. The file format lives in
//! [`jackdaw_env::extensions_config`] so tools without Bevy can edit it;
//! this module adds the editor-side snapshot of loaded extensions.

use bevy::{platform::collections::HashSet, prelude::*};

pub use jackdaw_env::extensions_config::*;

/// Compute the current enabled list from the loaded `Extension` entities
/// and write it to disk.
pub fn persist_current_enabled(world: &mut World) {
    let mut query = world.query::<&crate::lifecycle::Extension>();
    let mut enabled: HashSet<String> = query.iter(world).map(|e| e.id.clone()).collect();
    // An extension waiting on a restart is recorded as the user asked, so the
    // restart applies it.
    if let Some(app_plugins) = world.get_resource::<crate::lifecycle::ExtensionAppPlugins>() {
        for (id, wanted) in &app_plugins.pending {
            if *wanted {
                enabled.insert(id.clone());
            } else {
                enabled.remove(id);
            }
        }
    }
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
