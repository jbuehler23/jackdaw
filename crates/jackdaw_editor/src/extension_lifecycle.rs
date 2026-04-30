use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::enable_extension;

use crate::extension_resolution::resolve_enabled_list;

pub(super) fn plugin(app: &mut App) {
    // Must run after every plugin's `finish()`: BEI initializes
    // `ContextInstances<PreUpdate>` there, and spawning a context
    // entity before that resource exists panics.
    app.add_systems(Startup, apply_enabled_extensions_startup);
}

/// Enable every catalog entry `resolve_enabled_list` reports as on.
fn apply_enabled_extensions_startup(world: &mut World) {
    let to_enable = resolve_enabled_list(world);
    for name in &to_enable {
        enable_extension(world, name);
    }
}
