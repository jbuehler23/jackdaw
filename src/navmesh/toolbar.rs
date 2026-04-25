use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    separator, tokens,
};

use super::brp_client::NavmeshFetchOp;
use super::build::NavmeshBuildOp;
use super::ops::{
    NavmeshToggleDetailOp, NavmeshToggleObstaclesOp, NavmeshTogglePolyOp, NavmeshToggleVisualOp,
};
use super::save_load::{NavmeshLoadOp, NavmeshSaveOp};
use super::visualization::NavmeshVizConfig;
use crate::core_extension::ButtonPropsOpExt as _;
use crate::{EditorEntity, selection::Selection};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (toggle_toolbar_visibility, update_navmesh_viz_highlights)
            .run_if(in_state(crate::AppState::Editor)),
    );
    app.add_observer(on_viz_toggle_added);
}

/// Marker for the navmesh contextual toolbar node.
#[derive(Component)]
pub struct NavmeshToolbar;

/// Marker for navmesh visualization toggle buttons.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum NavmeshVizToggle {
    Visual,
    Obstacles,
    DetailMesh,
    PolygonMesh,
}

/// Builds the navmesh toolbar UI node. Starts hidden (`Display::None`).
pub fn navmesh_toolbar() -> impl Bundle {
    (
        NavmeshToolbar,
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
            column_gap: px(tokens::SPACING_SM),
            width: percent(100),
            height: px(32.0),
            flex_shrink: 0.0,
            display: Display::None,
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BG),
        children![
            (
                Text::new("Navmesh"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            button::button(
                ButtonProps::from_operator::<NavmeshFetchOp>().with_variant(ButtonVariant::Primary),
            ),
            button::button(ButtonProps::from_operator::<NavmeshBuildOp>()),
            button::button(ButtonProps::from_operator::<NavmeshSaveOp>()),
            button::button(ButtonProps::from_operator::<NavmeshLoadOp>()),
            // Separator before viz toggles
            separator::separator(separator::SeparatorProps::vertical()),
            // Visualization toggle buttons
            viz_toggle_button("Visual", NavmeshVizToggle::Visual, true),
            viz_toggle_button("Obstacles", NavmeshVizToggle::Obstacles, false),
            viz_toggle_button("Detail", NavmeshVizToggle::DetailMesh, true),
            viz_toggle_button("Poly", NavmeshVizToggle::PolygonMesh, false),
        ],
    )
}

fn viz_toggle_button(
    label: &str,
    toggle: NavmeshVizToggle,
    _initially_active: bool,
) -> impl Bundle {
    let op_id = match toggle {
        NavmeshVizToggle::Visual => NavmeshToggleVisualOp::ID,
        NavmeshVizToggle::Obstacles => NavmeshToggleObstaclesOp::ID,
        NavmeshVizToggle::DetailMesh => NavmeshToggleDetailOp::ID,
        NavmeshVizToggle::PolygonMesh => NavmeshTogglePolyOp::ID,
    };
    (
        toggle,
        button::button(
            ButtonProps::new(label)
                .with_variant(ButtonVariant::Default)
                .call_operator(op_id),
        ),
    )
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

fn update_navmesh_viz_highlights(
    viz_config: Res<NavmeshVizConfig>,
    mut buttons: Query<(&NavmeshVizToggle, &mut BackgroundColor)>,
) {
    if !viz_config.is_changed() {
        return;
    }

    for (toggle, mut bg) in &mut buttons {
        bg.0 = viz_toggle_bg(&viz_config, toggle);
    }
}

fn on_viz_toggle_added(
    trigger: On<Add, NavmeshVizToggle>,
    viz_config: Res<NavmeshVizConfig>,
    mut buttons: Query<(&NavmeshVizToggle, &mut BackgroundColor)>,
) {
    if let Ok((toggle, mut bg)) = buttons.get_mut(trigger.event_target()) {
        bg.0 = viz_toggle_bg(&viz_config, toggle);
    }
}

fn viz_toggle_bg(config: &NavmeshVizConfig, toggle: &NavmeshVizToggle) -> Color {
    let active = match toggle {
        NavmeshVizToggle::Visual => config.show_visual,
        NavmeshVizToggle::Obstacles => config.show_obstacles,
        NavmeshVizToggle::DetailMesh => config.show_detail_mesh,
        NavmeshVizToggle::PolygonMesh => config.show_polygon_mesh,
    };
    if active {
        tokens::SELECTED_BG
    } else {
        tokens::TOOLBAR_BUTTON_BG
    }
}
