use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonProps, ButtonVariant},
    separator, tokens,
};

use super::TerrainEditMode;
use super::ops::{
    TerrainToolFlattenOp, TerrainToolGenerateOp, TerrainToolLowerOp, TerrainToolNoiseOp,
    TerrainToolRaiseOp, TerrainToolSmoothOp,
};
use crate::{EditorEntity, selection::Selection};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (toggle_toolbar_visibility, update_terrain_tool_highlights)
            .run_if(in_state(crate::AppState::Editor)),
    );
}

/// Marker for the terrain contextual toolbar node.
#[derive(Component)]
pub struct TerrainToolbar;

/// Marker for terrain tool buttons to highlight the active tool.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum TerrainToolButton {
    Raise,
    Lower,
    Flatten,
    Smooth,
    Noise,
    Generate,
}

/// Builds the terrain toolbar UI node. Starts hidden (`Display::None`).
pub fn terrain_toolbar() -> impl Bundle {
    (
        TerrainToolbar,
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
            // Label
            (
                Text::new("Terrain"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            // Sculpt tools
            terrain_tool_button("Raise", TerrainToolButton::Raise),
            terrain_tool_button("Lower", TerrainToolButton::Lower),
            terrain_tool_button("Flatten", TerrainToolButton::Flatten),
            terrain_tool_button("Smooth", TerrainToolButton::Smooth),
            terrain_tool_button("Noise", TerrainToolButton::Noise),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            // Generate button
            terrain_tool_button("Generate", TerrainToolButton::Generate),
        ],
    )
}

fn terrain_tool_button(label: &str, tool: TerrainToolButton) -> impl Bundle {
    let op_id = match tool {
        TerrainToolButton::Raise => TerrainToolRaiseOp::ID,
        TerrainToolButton::Lower => TerrainToolLowerOp::ID,
        TerrainToolButton::Flatten => TerrainToolFlattenOp::ID,
        TerrainToolButton::Smooth => TerrainToolSmoothOp::ID,
        TerrainToolButton::Noise => TerrainToolNoiseOp::ID,
        TerrainToolButton::Generate => TerrainToolGenerateOp::ID,
    };
    (
        tool,
        button::button(
            ButtonProps::new(label)
                .with_variant(ButtonVariant::Default)
                .call_operator(op_id),
        ),
    )
}

fn toggle_toolbar_visibility(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_jsn::Terrain>>,
    mut toolbar: Query<&mut Node, With<TerrainToolbar>>,
    mut edit_mode: ResMut<TerrainEditMode>,
) {
    if !selection.is_changed() {
        return;
    }

    let should_show = selection.primary().is_some_and(|e| terrains.contains(e));

    for mut node in &mut toolbar {
        node.display = if should_show {
            Display::Flex
        } else {
            Display::None
        };
    }

    // Reset edit mode when terrain is deselected
    if !should_show && *edit_mode != TerrainEditMode::None {
        *edit_mode = TerrainEditMode::None;
    }
}

fn update_terrain_tool_highlights(
    edit_mode: Res<TerrainEditMode>,
    mut buttons: Query<(&TerrainToolButton, &mut BackgroundColor)>,
) {
    if !edit_mode.is_changed() {
        return;
    }

    for (button, mut bg) in &mut buttons {
        let active = matches!(
            (&*edit_mode, button),
            (
                TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Raise),
                TerrainToolButton::Raise
            ) | (
                TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Lower),
                TerrainToolButton::Lower
            ) | (
                TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Flatten),
                TerrainToolButton::Flatten
            ) | (
                TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Smooth),
                TerrainToolButton::Smooth
            ) | (
                TerrainEditMode::Sculpt(jackdaw_terrain::SculptTool::Noise),
                TerrainToolButton::Noise
            ) | (TerrainEditMode::Generate, TerrainToolButton::Generate)
        );
        bg.0 = if active {
            tokens::SELECTED_BG
        } else {
            tokens::TOOLBAR_BUTTON_BG
        };
    }
}
