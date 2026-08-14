use bevy::feathers::controls::{ButtonVariant, FeathersToolButton};
use bevy::feathers::display::label_dim;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSourceTemplate};
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::{Icon, font_paths};
use jackdaw_feathers::tokens;

use super::TerrainEditMode;
use super::ops::{
    TerrainToolFlattenOp, TerrainToolGenerateOp, TerrainToolLowerOp, TerrainToolNoiseOp,
    TerrainToolRaiseOp, TerrainToolSmoothOp,
};
use super::paint::TerrainToolPaintOp;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (toggle_toolbar_visibility, update_active_tool_description)
            .run_if(in_state(crate::AppState::Editor)),
    )
    .add_observer(update_terrain_tool_highlights);
}

/// Icon size in the terrain toolbar. Matches the main toolbar's, so the
/// two rows read as one control surface rather than two.
const TOOL_ICON_PX: f32 = 16.0;

/// Marker for the terrain contextual toolbar node.
#[derive(Component)]
pub struct TerrainToolbar;

/// Marker on the text node that describes the active terrain tool.
///
/// Every reference tool pairs its icon row with a one-line description of
/// the selected tool -- Unity puts "Paints the selected material layer
/// onto the terrain texture" directly under its five icons. Each terrain
/// operator already carries a `description`; this surfaces it.
#[derive(Component, Clone, Default)]
struct TerrainToolDescription;

/// Builds the terrain toolbar as a `bsn!` Scene. Starts hidden
/// (`Display::None`).
///
/// Spawned standalone via `spawn_scene`; see
/// `viewport::build_viewport_panel`. A Scene can't nest inside a Bundle
/// `children!` tree, and the spawn site attaches the [`TerrainToolbar`]
/// and `EditorEntity` markers. Each button is a `tool_button` carrying a
/// `ButtonOperatorCall`. The editor's operator-button glue dispatches the
/// op on `Activate` and greys it out when unavailable.
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
            tool_button(TerrainToolRaiseOp::ID, Icon::ArrowUpFromLine),
            tool_button(TerrainToolLowerOp::ID, Icon::ArrowDownToLine),
            tool_button(TerrainToolFlattenOp::ID, Icon::Minimize2),
            tool_button(TerrainToolSmoothOp::ID, Icon::Spline),
            tool_button(TerrainToolNoiseOp::ID, Icon::Waves),
            vertical_separator(),
            tool_button(TerrainToolPaintOp::ID, Icon::Paintbrush),
            vertical_separator(),
            tool_button(TerrainToolGenerateOp::ID, Icon::Mountain),
            vertical_separator(),
            active_tool_description(),
        ]
    }
}

/// One terrain tool: a Lucide glyph in a `FeathersToolButton`, matching
/// the main toolbar's idiom exactly rather than the text-label buttons
/// this row used to carry. The operator's `label` and `description` reach
/// the user through the hover tooltip the operator-button glue attaches
/// and through the description line below the row.
fn tool_button(op_id: &'static str, icon: Icon) -> impl Scene {
    let glyph = String::from(icon.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! {
                Text(glyph)
                TextFont {
                    font: FontSourceTemplate::Handle(font_paths::LUCIDE),
                    font_size: FontSize::Px(TOOL_ICON_PX),
                }
            },
            @variant: {ButtonVariant::Plain}
        }
        ButtonOperatorCall::new(op_id)
    }
}

fn active_tool_description() -> impl Scene {
    bsn! {
        Text("")
        TextFont { font_size: {tokens::TEXT_SIZE_XS} }
        TextColor({tokens::TEXT_SECONDARY})
        TerrainToolDescription
    }
}

/// Human-readable summary of what the active tool does, in the same
/// register as Unity's one-liner under its tool icons.
fn describe(mode: &TerrainEditMode) -> &'static str {
    use jackdaw_terrain::SculptTool;
    match mode {
        TerrainEditMode::None => "Pick a tool to start editing this terrain.",
        TerrainEditMode::Sculpt(SculptTool::Raise) => "Raises the terrain under the brush.",
        TerrainEditMode::Sculpt(SculptTool::Lower) => "Lowers the terrain under the brush.",
        TerrainEditMode::Sculpt(SculptTool::Flatten) => {
            "Levels the terrain under the brush toward its starting height."
        }
        TerrainEditMode::Sculpt(SculptTool::Smooth) => {
            "Averages the terrain under the brush, softening hard edges."
        }
        TerrainEditMode::Sculpt(SculptTool::Noise) => "Adds fine random detail under the brush.",
        TerrainEditMode::Paint => {
            "Paints the selected palette value into the active channel. Hold Ctrl to erase."
        }
        TerrainEditMode::Generate => "Builds a whole heightmap from noise settings.",
    }
}

fn update_active_tool_description(
    edit_mode: Res<TerrainEditMode>,
    mut labels: Query<&mut Text, With<TerrainToolDescription>>,
) {
    if !edit_mode.is_changed() {
        return;
    }
    let text = describe(&edit_mode);
    for mut label in &mut labels {
        if label.0 != text {
            label.0 = text.to_string();
        }
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
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
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
    } else if op_id == TerrainToolPaintOp::ID {
        Some(*mode == TerrainEditMode::Paint)
    } else if op_id == TerrainToolGenerateOp::ID {
        Some(*mode == TerrainEditMode::Generate)
    } else {
        None
    }
}
