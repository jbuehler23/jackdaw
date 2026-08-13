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
        .register_operator::<GridSetIncrementOp>()
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

/// Stepping the power takes the grid back onto the power-of-two ladder.
///
/// The power control and the explicit increment are two ways of saying
/// the same thing, so touching one has to win outright. Leaving a 1.5
/// increment in place while the power stepped underneath it would make
/// the bracket keys look broken.
#[operator(id = "grid.increase", label = "Increase Grid")]
pub(crate) fn grid_increase(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_increment = 0.0;
    snap.grid_power = i32::min(snap.grid_power + 1, GRID_POWER_MAX);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}

#[operator(id = "grid.decrease", label = "Decrease Grid")]
pub(crate) fn grid_decrease(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_increment = 0.0;
    snap.grid_power = i32::max(snap.grid_power - 1, GRID_POWER_MIN);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}

/// Set the grid to an explicit size, off the power-of-two ladder.
///
/// A value of `0` or less hands the grid back to `grid_power`, so there
/// is a way out that does not require guessing which power the game was
/// on before.
#[operator(
    id = "grid.set_increment",
    label = "Set Grid Increment",
    description = "Set the grid to an exact size in world units.",
    params(value(
        f64,
        doc = "Grid size in world units. 0 or less restores the power-of-two grid."
    ))
)]
pub(crate) fn grid_set_increment(
    params: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    let value = params.as_float("value")? as f32;
    snap.grid_increment = if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    };
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
