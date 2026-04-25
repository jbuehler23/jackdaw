//! Operators for the navmesh contextual toolbar. The fetch / build /
//! save / load ops dispatch existing events; the four viz toggles
//! flip flags on `NavmeshVizConfig`.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::brp_client::GetNavmeshInput;
use super::build::BuildNavmesh;
use super::save_load::{LoadNavmesh, SaveNavmesh};
use super::visualization::NavmeshVizConfig;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<NavmeshFetchOp>()
        .register_operator::<NavmeshBuildOp>()
        .register_operator::<NavmeshSaveOp>()
        .register_operator::<NavmeshLoadOp>()
        .register_operator::<NavmeshToggleVisualOp>()
        .register_operator::<NavmeshToggleObstaclesOp>()
        .register_operator::<NavmeshToggleDetailOp>()
        .register_operator::<NavmeshTogglePolyOp>();
}

#[operator(
    id = "navmesh.fetch",
    label = "Fetch Scene",
    description = "Request the active scene's navmesh input from the connected game.",
    allows_undo = false
)]
pub(crate) fn navmesh_fetch(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(GetNavmeshInput);
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.build",
    label = "Build",
    description = "Build the navmesh from the current scene input.",
    allows_undo = false
)]
pub(crate) fn navmesh_build(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(BuildNavmesh);
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.save",
    label = "Save",
    description = "Write the current navmesh to disk.",
    allows_undo = false
)]
pub(crate) fn navmesh_save(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(SaveNavmesh);
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.load",
    label = "Load",
    description = "Load a navmesh from disk.",
    allows_undo = false
)]
pub(crate) fn navmesh_load(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(LoadNavmesh);
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.toggle_visual",
    label = "Toggle Visual",
    description = "Toggle the navmesh visual-mesh overlay.",
    allows_undo = false
)]
pub(crate) fn navmesh_toggle_visual(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_visual = !config.show_visual;
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.toggle_obstacles",
    label = "Toggle Obstacles",
    description = "Toggle the navmesh obstacle gizmo overlay.",
    allows_undo = false
)]
pub(crate) fn navmesh_toggle_obstacles(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_obstacles = !config.show_obstacles;
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.toggle_detail",
    label = "Toggle Detail Mesh",
    description = "Toggle the navmesh detail-mesh overlay.",
    allows_undo = false
)]
pub(crate) fn navmesh_toggle_detail(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_detail_mesh = !config.show_detail_mesh;
    OperatorResult::Finished
}

#[operator(
    id = "navmesh.toggle_poly",
    label = "Toggle Polygon Mesh",
    description = "Toggle the navmesh polygon-mesh overlay.",
    allows_undo = false
)]
pub(crate) fn navmesh_toggle_poly(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_polygon_mesh = !config.show_polygon_mesh;
    OperatorResult::Finished
}
