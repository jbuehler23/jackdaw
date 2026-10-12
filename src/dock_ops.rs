//! Operators for the panel docking system.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_panels::PanelGroup;
use jackdaw_panels::area::{DockTab, DockTabCloseButton};
use jackdaw_panels::reconcile::NodeBinding;
use jackdaw_panels::tree::{DockNode, DockTree, NodeId, SplitAxis, TabId};

pub struct DockOpsPlugin;

impl Plugin for DockOpsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_close_button_click)
            .add_observer(on_tab_middle_click);
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<DockCloseTabOp>();
    ctx.register_operator::<WindowResizePanelOp>();
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

/// The smallest share of a split either side keeps, matching the clamp
/// `DockTree::set_fraction` applies.
const MIN_SHARE: f32 = 0.05;

#[operator(
    id = "window.resize_panel",
    label = "Resize Panel",
    description = "Size the dock panel a window sits in, in logical pixels.",
    allows_undo = false,
    params(
        window_id(
            String,
            doc = "Window the panel holds, such as \"jackdaw.inspector.components\", \
                   or the dock area it sits in, such as \"right_sidebar\"."
        ),
        width(
            f64,
            doc = "Width in logical pixels, for a panel beside its neighbour."
        ),
        height(
            f64,
            doc = "Height in logical pixels, for a panel above or below its neighbour."
        ),
    )
)]
pub(crate) fn window_resize_panel(
    In(params): In<OperatorParameters>,
    mut tree: ResMut<DockTree>,
    groups: Query<(&NodeBinding, &ComputedNode), With<PanelGroup>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(window_id) = params.as_str("window_id") else {
        refuse(&mut commands, "missing 'window_id' parameter".to_string());
        return OperatorResult::Cancelled;
    };
    let Some(leaf) = tree
        .find_leaf_with_window(window_id)
        .or_else(|| tree.find_by_area_id(window_id))
    else {
        refuse(
            &mut commands,
            format!(
                "no dock panel holds '{window_id}'; this layout has {:?}",
                panels_on_offer(&tree)
            ),
        );
        return OperatorResult::Cancelled;
    };
    let Some(split_id) = tree.parent_of(leaf) else {
        refuse(
            &mut commands,
            format!("'{window_id}' fills the layout and has nothing to share with"),
        );
        return OperatorResult::Cancelled;
    };
    let Some(split) = tree.get(split_id).and_then(DockNode::as_split) else {
        return OperatorResult::Cancelled;
    };
    let axis = split.axis;
    let leads = split.a == leaf;

    let wanted = match axis {
        SplitAxis::Horizontal => params.as_float("width"),
        SplitAxis::Vertical => params.as_float("height"),
    };
    let Some(wanted) = wanted.filter(|size| *size > 0.0) else {
        let asked = match axis {
            SplitAxis::Horizontal => "width",
            SplitAxis::Vertical => "height",
        };
        refuse(
            &mut commands,
            format!("'{window_id}' is sized by its {asked}"),
        );
        return OperatorResult::Cancelled;
    };

    let total = split_extent(split_id, axis, &groups, &windows);
    if total <= 0.0 {
        refuse(
            &mut commands,
            "the layout has no size to divide yet".to_string(),
        );
        return OperatorResult::Cancelled;
    }
    let share = (wanted as f32 / total).clamp(MIN_SHARE, 1.0 - MIN_SHARE);
    tree.set_fraction(split_id, if leads { share } else { 1.0 - share });
    OperatorResult::Finished
}

/// Tell the caller why the panel was not sized, which a caller with no log to
/// read has no other way of learning.
fn refuse(commands: &mut Commands, reason: String) {
    commands.queue(move |world: &mut World| {
        warn_caller(world, format!("window.resize_panel: {reason}"));
    });
}

/// Every panel a caller could have named: each dock area and the windows in it.
fn panels_on_offer(tree: &DockTree) -> Vec<String> {
    tree.leaves()
        .map(|(_, leaf)| {
            let windows: Vec<&str> = leaf.tabs().map(|(window_id, _)| window_id).collect();
            format!("{} {windows:?}", leaf.area_id)
        })
        .collect()
}

/// How many logical pixels a split has to divide: what the laid-out container
/// measures, and the window it lives in before the first layout pass.
fn split_extent(
    split: NodeId,
    axis: SplitAxis,
    groups: &Query<(&NodeBinding, &ComputedNode), With<PanelGroup>>,
    windows: &Query<&Window, With<bevy::window::PrimaryWindow>>,
) -> f32 {
    let along = |size: Vec2| match axis {
        SplitAxis::Horizontal => size.x,
        SplitAxis::Vertical => size.y,
    };
    groups
        .iter()
        .find(|(binding, _)| binding.0 == split)
        .map(|(_, computed)| along(computed.size() * computed.inverse_scale_factor()))
        .filter(|extent| *extent > 0.0)
        .or_else(|| {
            windows
                .single()
                .ok()
                .map(|window| along(Vec2::new(window.width(), window.height())))
        })
        .unwrap_or_default()
}

fn on_close_button_click(
    trigger: On<PointerClick>,
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

fn on_tab_middle_click(trigger: On<PointerClick>, tabs: Query<&DockTab>, mut commands: Commands) {
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
