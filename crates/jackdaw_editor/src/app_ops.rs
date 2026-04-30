//! App-level operators: open the Extensions dialog, open the Keybind
//! settings dialog, toggle hot reload, return to the project-select
//! home screen. None have keybinds currently; they exist so menus (and
//! a future command palette) can dispatch them uniformly.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<AppOpenExtensionsOp>()
        .register_operator::<AppOpenKeybindsOp>()
        .register_operator::<AppToggleHotReloadOp>()
        .register_operator::<AppGoHomeOp>();
}

#[operator(
    id = "app.open_extensions",
    label = "Extensions...",
    allows_undo = false
)]
pub(crate) fn app_open_extensions(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        crate::extensions_dialog::open_extensions_dialog(world);
    });
    OperatorResult::Finished
}

#[operator(id = "app.open_keybinds", label = "Keybinds...", allows_undo = false)]
pub(crate) fn app_open_keybinds(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.trigger(crate::keybind_settings::OpenKeybindSettingsEvent);
    OperatorResult::Finished
}

#[operator(
    id = "app.toggle_hot_reload",
    label = "Toggle Hot Reload",
    allows_undo = false
)]
pub(crate) fn app_toggle_hot_reload(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let mut enabled = world.resource_mut::<crate::hot_reload::HotReloadEnabled>();
        enabled.0 = !enabled.0;
        let state = if enabled.0 { "on" } else { "off" };
        info!("Hot reload toggled {state}");
        // Menu label reflects on/off state; flag so populate_menu_bar
        // re-runs and picks up the new label.
        world.resource_mut::<crate::MenuBarDirty>().0 = true;
    });
    OperatorResult::Finished
}

#[operator(id = "app.go_home", label = "Home", allows_undo = false)]
pub(crate) fn app_go_home(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        world
            .resource_mut::<NextState<crate::AppState>>()
            .set(crate::AppState::ProjectSelect);
    });
    OperatorResult::Finished
}
