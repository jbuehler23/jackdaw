//! Operators for the panel docking system.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_panels::area::{DockTab, DockTabCloseButton};
use jackdaw_panels::tree::{DockTree, TabId};

pub struct DockOpsPlugin;

impl Plugin for DockOpsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_close_button_click)
            .add_observer(on_tab_middle_click);
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<DockCloseTabOp>();
}

#[operator(
    id = "dock.close_tab",
    label = "Close Tab",
    description = "Close the specified docked tab.",
    allows_undo = false,
    params(tab_id(i64, doc = "TabId of the tab to close."))
)]
pub(crate) fn dock_close_tab(
    In(params): In<OperatorParameters>,
    mut tree: ResMut<DockTree>,
) -> OperatorResult {
    let Some(tab_id) = params.as_int("tab_id") else {
        warn!("dock.close_tab: missing 'tab_id' parameter");
        return OperatorResult::Cancelled;
    };
    tree.remove_tab(TabId(tab_id as u64));
    OperatorResult::Finished
}

fn on_close_button_click(
    trigger: On<Pointer<Click>>,
    close_buttons: Query<&DockTabCloseButton>,
    mut commands: Commands,
) {
    let Ok(close_btn) = close_buttons.get(trigger.event_target()) else {
        return;
    };
    commands
        .operator(DockCloseTabOp::ID)
        .param("tab_id", close_btn.tab_id.0 as i64)
        .call();
}

fn on_tab_middle_click(trigger: On<Pointer<Click>>, tabs: Query<&DockTab>, mut commands: Commands) {
    if trigger.event().button != PointerButton::Middle {
        return;
    }
    let Ok(tab) = tabs.get(trigger.event_target()) else {
        return;
    };
    commands
        .operator(DockCloseTabOp::ID)
        .param("tab_id", tab.tab_id.0 as i64)
        .call();
}
