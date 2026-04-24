//! Grid-size operators: increase / decrease the editor grid by one
//! power. Flips a resource, no history entry.
//!
//! Default keybinds: `]` (increase), `[` (decrease). The scroll-wheel
//! path for grid resize lives alongside the modifier-gated scroll
//! handler in [`crate::snapping`].

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;
use crate::snapping::{GRID_POWER_MAX, GRID_POWER_MIN, SnapSettings};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GridIncreaseOp>()
        .register_operator::<GridDecreaseOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<GridIncreaseOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::BracketRight, Press::default())],
        ));
        world.spawn((
            Action::<GridDecreaseOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::BracketLeft, Press::default())],
        ));
    });
}

#[operator(id = "grid.increase", label = "Increase Grid", allows_undo = false)]
pub(crate) fn grid_increase(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_power = i32::min(snap.grid_power + 1, GRID_POWER_MAX);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}

#[operator(id = "grid.decrease", label = "Decrease Grid", allows_undo = false)]
pub(crate) fn grid_decrease(
    _: In<OperatorParameters>,
    mut snap: ResMut<SnapSettings>,
) -> OperatorResult {
    snap.grid_power = i32::max(snap.grid_power - 1, GRID_POWER_MIN);
    snap.translate_increment = snap.grid_size();
    OperatorResult::Finished
}
