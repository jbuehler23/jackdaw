use bevy::feathers::controls::ButtonVariant;
use bevy::feathers::display::label_dim;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{ButtonOperatorCall, operator_button};
use jackdaw_feathers::tokens;

use super::TerrainEditMode;
use super::ops::{
    TerrainToolFlattenOp, TerrainToolGenerateOp, TerrainToolLowerOp, TerrainToolNoiseOp,
    TerrainToolRaiseOp, TerrainToolSmoothOp,
};
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        toggle_toolbar_visibility.run_if(in_state(crate::AppState::Editor)),
    )
    .add_observer(update_terrain_tool_highlights);
}

/// Marker for the terrain contextual toolbar node.
#[derive(Component)]
pub struct TerrainToolbar;

/// Builds the terrain toolbar as a `bsn!` Scene. Starts hidden
/// (`Display::None`).
///
/// Spawned standalone via `spawn_scene`; see
/// `viewport::build_viewport_panel`. A Scene can't nest inside a Bundle
/// `children!` tree, and the spawn site attaches the [`TerrainToolbar`]
/// and `EditorEntity` markers. Each button is an [`operator_button`]
/// carrying a `ButtonOperatorCall`. The editor's operator-button glue
/// dispatches the op on `Activate` and greys it out when unavailable.
pub fn terrain_toolbar() -> impl Scene {
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
            label_dim("Terrain"),
            operator_button(TerrainToolRaiseOp::ID, TerrainToolRaiseOp::LABEL),
            operator_button(TerrainToolLowerOp::ID, TerrainToolLowerOp::LABEL),
            operator_button(TerrainToolFlattenOp::ID, TerrainToolFlattenOp::LABEL),
            operator_button(TerrainToolSmoothOp::ID, TerrainToolSmoothOp::LABEL),
            operator_button(TerrainToolNoiseOp::ID, TerrainToolNoiseOp::LABEL),
            vertical_separator(),
            operator_button(TerrainToolGenerateOp::ID, TerrainToolGenerateOp::LABEL),
        ]
    }
}

/// Thin vertical rule separating the sculpt tools from the generate
/// action.
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

/// Highlight the active terrain tool by flipping its [`ButtonVariant`] to
/// `Primary`. Tools are identified by operator id rather than a dedicated
/// marker. Buttons whose ids don't belong to a terrain tool are skipped,
/// so the main toolbar's own highlighter keeps ownership of its buttons.
///
/// This is an [`On<RefreshOperatorButtons>`] observer. The terrain tool
/// ops mutate [`TerrainEditMode`] through operator dispatch, which
/// announces the refresh, and a freshly-spawned toolbar seeds off the same
/// event fired when each button is added. The loop is O(operator buttons)
/// and only writes on an actual change.
fn update_terrain_tool_highlights(
    _: On<RefreshOperatorButtons>,
    edit_mode: Res<TerrainEditMode>,
    mut buttons: Query<(&ButtonOperatorCall, &mut ButtonVariant)>,
) {
    for (call, mut variant) in &mut buttons {
        let Some(active) = terrain_tool_active(&edit_mode, call.id.as_ref()) else {
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

/// Map a terrain-tool operator id to whether its tool is the active
/// edit mode. Returns `None` for any id this toolbar doesn't own (e.g.
/// the main toolbar's tool buttons), which the highlighter then leaves
/// alone.
fn terrain_tool_active(mode: &TerrainEditMode, op_id: &str) -> Option<bool> {
    use jackdaw_terrain::SculptTool;
    if op_id == TerrainToolRaiseOp::ID {
        Some(matches!(mode, TerrainEditMode::Sculpt(SculptTool::Raise)))
    } else if op_id == TerrainToolLowerOp::ID {
        Some(matches!(mode, TerrainEditMode::Sculpt(SculptTool::Lower)))
    } else if op_id == TerrainToolFlattenOp::ID {
        Some(matches!(mode, TerrainEditMode::Sculpt(SculptTool::Flatten)))
    } else if op_id == TerrainToolSmoothOp::ID {
        Some(matches!(mode, TerrainEditMode::Sculpt(SculptTool::Smooth)))
    } else if op_id == TerrainToolNoiseOp::ID {
        Some(matches!(mode, TerrainEditMode::Sculpt(SculptTool::Noise)))
    } else if op_id == TerrainToolGenerateOp::ID {
        Some(*mode == TerrainEditMode::Generate)
    } else {
        None
    }
}
