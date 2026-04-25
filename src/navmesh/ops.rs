//! Operators for the navmesh contextual toolbar.
//!
//! Toggle ops live here; the action ops (fetch, build, save, load) are
//! defined alongside their state in [`super::brp_client`],
//! [`super::build`], and [`super::save_load`].

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::brp_client::NavmeshFetchOp;
use super::build::NavmeshBuildOp;
use super::save_load::{NavmeshLoadOp, NavmeshSaveOp};
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
