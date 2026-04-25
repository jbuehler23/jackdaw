//! Operators for the navmesh contextual toolbar.

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

/// Fetch the latest scene mesh from the connected game so the navmesh
/// can be rebuilt from it.
#[operator(
    id = "navmesh.fetch",
    label = "Fetch Scene",
    description = "Fetch the latest scene mesh from the connected game."
)]
pub(crate) fn navmesh_fetch(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(GetNavmeshInput);
    OperatorResult::Finished
}

/// Bake a navmesh for the current scene.
#[operator(
    id = "navmesh.build",
    label = "Build",
    description = "Bake a navmesh for the current scene."
)]
pub(crate) fn navmesh_build(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(BuildNavmesh);
    OperatorResult::Finished
}

/// Save the baked navmesh to disk.
#[operator(
    id = "navmesh.save",
    label = "Save",
    description = "Save the baked navmesh to disk."
)]
pub(crate) fn navmesh_save(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(SaveNavmesh);
    OperatorResult::Finished
}

/// Load a navmesh from disk.
#[operator(
    id = "navmesh.load",
    label = "Load",
    description = "Load a navmesh from disk."
)]
pub(crate) fn navmesh_load(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(LoadNavmesh);
    OperatorResult::Finished
}

/// Show or hide the navmesh visual mesh.
#[operator(
    id = "navmesh.toggle_visual",
    label = "Toggle Visual",
    description = "Show or hide the navmesh visual mesh."
)]
pub(crate) fn navmesh_toggle_visual(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_visual = !config.show_visual;
    OperatorResult::Finished
}

/// Show or hide the navmesh obstacle markers.
#[operator(
    id = "navmesh.toggle_obstacles",
    label = "Toggle Obstacles",
    description = "Show or hide the navmesh obstacle markers."
)]
pub(crate) fn navmesh_toggle_obstacles(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_obstacles = !config.show_obstacles;
    OperatorResult::Finished
}

/// Show or hide the navmesh detail mesh.
#[operator(
    id = "navmesh.toggle_detail",
    label = "Toggle Detail Mesh",
    description = "Show or hide the navmesh detail mesh."
)]
pub(crate) fn navmesh_toggle_detail(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_detail_mesh = !config.show_detail_mesh;
    OperatorResult::Finished
}

/// Show or hide the navmesh polygon mesh.
#[operator(
    id = "navmesh.toggle_poly",
    label = "Toggle Polygon Mesh",
    description = "Show or hide the navmesh polygon mesh."
)]
pub(crate) fn navmesh_toggle_poly(
    _: In<OperatorParameters>,
    mut config: ResMut<NavmeshVizConfig>,
) -> OperatorResult {
    config.show_polygon_mesh = !config.show_polygon_mesh;
    OperatorResult::Finished
}
