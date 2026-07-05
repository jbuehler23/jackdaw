use bevy::feathers::controls::ButtonVariant;
use bevy::feathers::display::label_dim;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{ButtonOperatorCall, operator_button, operator_button_variant};
use jackdaw_feathers::tokens;

use super::brp_client::NavmeshFetchOp;
use super::build::NavmeshBuildOp;
use super::ops::{
    NavmeshToggleDetailOp, NavmeshToggleObstaclesOp, NavmeshTogglePolyOp, NavmeshToggleVisualOp,
};
use super::save_load::{NavmeshLoadOp, NavmeshSaveOp};
use super::visualization::NavmeshVizConfig;

use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        toggle_toolbar_visibility.run_if(in_state(crate::AppState::Editor)),
    )
    .add_observer(update_navmesh_viz_highlights);
}

/// Marker for the navmesh contextual toolbar node.
#[derive(Component)]
pub struct NavmeshToolbar;

/// Builds the navmesh toolbar as a `bsn!` Scene. Starts hidden
/// (`Display::None`).
///
/// Spawned standalone via `spawn_scene`; see `viewport::setup_viewport`.
/// A Scene can't nest inside a Bundle `children!` tree, and the spawn
/// site attaches the [`NavmeshToolbar`] and `EditorEntity` markers. Each
/// button is an [`operator_button`] carrying a `ButtonOperatorCall`. The
/// editor's operator-button glue dispatches the op on `Activate` and
/// greys it out when unavailable.
pub fn navmesh_toolbar() -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
            column_gap: px(tokens::SPACING_SM),
            width: percent(100),
            height: px(32.0),
            flex_shrink: 0.0,
            display: Display::None,
        }
        BackgroundColor(tokens::TOOLBAR_BG)
        Children [
            label_dim("Navmesh"),
            operator_button_variant(NavmeshFetchOp::ID, NavmeshFetchOp::LABEL, ButtonVariant::Primary),
            operator_button(NavmeshBuildOp::ID, NavmeshBuildOp::LABEL),
            operator_button(NavmeshSaveOp::ID, NavmeshSaveOp::LABEL),
            operator_button(NavmeshLoadOp::ID, NavmeshLoadOp::LABEL),
            vertical_separator(),
            operator_button(NavmeshToggleVisualOp::ID, "Visual"),
            operator_button(NavmeshToggleObstaclesOp::ID, "Obstacles"),
            operator_button(NavmeshToggleDetailOp::ID, "Detail"),
            operator_button(NavmeshTogglePolyOp::ID, "Poly"),
        ]
    }
}

/// Thin vertical rule separating the action buttons from the
/// visualization toggles.
fn vertical_separator() -> impl Scene {
    let color: Color = tokens::TEXT_BODY_COLOR.with_alpha(0.1).into();
    bsn! {
        Node {
            width: px(1),
            align_self: AlignSelf::Stretch,
            margin: UiRect::vertical(px(6)),
        }
        BackgroundColor(color)
    }
}

fn toggle_toolbar_visibility(
    selection: Res<Selection>,
    regions: Query<(), With<jackdaw_jsn::NavmeshRegion>>,
    mut toolbar: Query<&mut Node, With<NavmeshToolbar>>,
) {
    if !selection.is_changed() {
        return;
    }

    let should_show = selection.primary().is_some_and(|e| regions.contains(e));

    for mut node in &mut toolbar {
        node.display = if should_show {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Highlight the active visualization toggles by flipping their
/// [`ButtonVariant`] to `Primary`. Toggles are identified by operator id
/// rather than a dedicated marker. Action buttons whose ids don't match a
/// toggle are skipped, so the `Primary` Fetch button keeps its variant.
///
/// This is an [`On<RefreshOperatorButtons>`] observer. The toggle ops
/// mutate the [`NavmeshVizConfig`] through operator dispatch, which
/// announces the refresh, and a freshly-spawned toolbar seeds off the same
/// event fired when each button is added. The loop is O(operator buttons)
/// and only writes on an actual change.
fn update_navmesh_viz_highlights(
    _: On<RefreshOperatorButtons>,
    viz_config: Res<NavmeshVizConfig>,
    mut buttons: Query<(&ButtonOperatorCall, &mut ButtonVariant)>,
) {
    for (call, mut variant) in &mut buttons {
        let Some(active) = viz_toggle_active(&viz_config, call.id.as_ref()) else {
            continue;
        };
        let target = if active {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Normal
        };
        if *variant != target {
            *variant = target;
        }
    }
}

/// Map a toggle operator id to whether its layer is currently shown.
/// Returns `None` for any non-toggle id (e.g. fetch/build/save/load),
/// which the highlighter then leaves alone.
fn viz_toggle_active(config: &NavmeshVizConfig, op_id: &str) -> Option<bool> {
    if op_id == NavmeshToggleVisualOp::ID {
        Some(config.show_visual)
    } else if op_id == NavmeshToggleObstaclesOp::ID {
        Some(config.show_obstacles)
    } else if op_id == NavmeshToggleDetailOp::ID {
        Some(config.show_detail_mesh)
    } else if op_id == NavmeshTogglePolyOp::ID {
        Some(config.show_polygon_mesh)
    } else {
        None
    }
}
