//! Grid-size operators: increase / decrease the editor grid by one
//! power, plus the master snap toggle. Flips a resource, no history
//! entry.
//!
//! Default keybinds: `]` (increase), `[` (decrease), `M` (toggle
//! snapping, same as the toolbar magnet). The scroll-wheel path for
//! grid resize lives alongside the modifier-gated scroll handler in
//! [`crate::snapping`].

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::{GRID_POWER_MAX, GRID_POWER_MIN, SnapSettings};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GridIncreaseOp>()
        .register_operator::<GridDecreaseOp>()
        .register_operator::<GridToggleSnapOp>();

    ctx.bind_operator::<CoreExtensionInputContext, GridIncreaseOp>([PresetInput::key(
        "BracketRight",
    )]);
    ctx.bind_operator::<CoreExtensionInputContext, GridDecreaseOp>([PresetInput::key(
        "BracketLeft",
    )]);
    // `M` for the magnet (snap) toggle, mirroring the toolbar magnet button.
    ctx.bind_operator::<CoreExtensionInputContext, GridToggleSnapOp>([PresetInput::key("KeyM")]);
}

#[operator(id = "grid.increase", label = "Increase Grid")]
pub(crate) fn grid_increase(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_power = i32::min(snap.grid_power + 1, GRID_POWER_MAX);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}

#[operator(id = "grid.decrease", label = "Decrease Grid")]
pub(crate) fn grid_decrease(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_power = i32::max(snap.grid_power - 1, GRID_POWER_MIN);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}

#[operator(id = "snap.toggle", label = "Toggle Snapping")]
pub(crate) fn grid_toggle_snap(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    // Master toggle: flip every snap type together so the magnet reads
    // as a single on/off. `translate_snap` is the representative state.
    let enabled = !snap.translate_snap;
    snap.translate_snap = enabled;
    snap.rotate_snap = enabled;
    snap.scale_snap = enabled;
    OperatorResult::Finished
}
