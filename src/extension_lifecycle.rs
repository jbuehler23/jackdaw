use bevy::prelude::*;
use jackdaw_api_internal::keymap::{
    ActiveKeymapPreset, BuiltinActions, DefaultKeymap, apply_keymap_preset,
};
use jackdaw_api_internal::lifecycle::enable_extension;

use crate::extension_resolution::resolve_enabled_list;
use crate::input_contexts::spawn_contexts;

pub(super) fn plugin(app: &mut App) {
    // Must run after every plugin's `finish()`: BEI initializes
    // `ContextInstances<PreUpdate>` there, and spawning a context
    // entity before that resource exists panics.
    //
    // Ordering guarantee: `spawn_contexts` runs before `apply_active_keymap`
    // so the `BuiltinActions` and `DefaultKeymap` entries for modal/nav are
    // present when the applier iterates preset bindings.
    //
    // `apply_active_keymap` chains after `apply_enabled_extensions_startup`
    // so extensions have registered all DefaultKeymap entries before
    // bindings are applied.
    app.init_resource::<BuiltinActions>()
        .init_resource::<DefaultKeymap>()
        .add_systems(
            Startup,
            (
                apply_enabled_extensions_startup,
                spawn_contexts,
                apply_active_keymap,
            )
                .chain(),
        );
}

/// Enable every catalog entry `resolve_enabled_list` reports as on.
fn apply_enabled_extensions_startup(world: &mut World) {
    let to_enable = resolve_enabled_list(world);
    for name in &to_enable {
        enable_extension(world, name);
    }
}

/// Apply the active keymap preset once extensions finish registering.
/// Only "classic" ships today; unknown names warn and fall back.
pub(crate) fn apply_active_keymap(world: &mut World) {
    let defaults = world
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset();
    let active = world.get_resource_or_init::<ActiveKeymapPreset>().clone();
    if active.name != "classic" {
        warn!(
            "unknown keymap preset '{}'; falling back to classic",
            active.name
        );
    }
    let report = apply_keymap_preset(world, &defaults);
    info!(
        "applied keymap preset 'classic': {} entries, {} bindings",
        report.applied_entries, report.spawned_bindings
    );
}
