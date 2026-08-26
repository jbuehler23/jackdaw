//! The vertical tool palette overlaid on the viewport's left edge.
//!
//! One button per operation (Raise, Lower, Flatten, Smooth, Noise, Paint,
//! Quantize, Navmesh, Regions), visible only while a Terrain entity is
//! selected. There is no Select entry: the viewport's main-toolbar Select tool
//! covers leaving a terrain tool, and pressing an active tool's own button
//! again puts it away (see `toggle_to` in `ops.rs`).
//!
//! `jackdaw_panels::area::DefaultArea` has no anchor for pinning to the edge of
//! another panel's content (only Left/Center/BottomDock/`RightSidebar`), so
//! this is a plain UI node, absolutely positioned, spawned as a child of the
//! viewport panel's content column by `viewport::build_viewport_panel`.
//!
//! It hangs off that column rather than off the `SceneViewport` node so its
//! position cannot depend on the contextual options bar sitting between them:
//! the bar is as tall as the active tool's own fields, and a palette anchored
//! below it would move whenever the tool changed. The bar keeps
//! `PALETTE_GUTTER_PX` clear on its left instead, so however many rows it
//! grows to, the two do not overlap.

use bevy::feathers::controls::{ButtonVariant, FeathersToolButton};
use bevy::prelude::*;
use bevy::text::{FontSize, FontSourceTemplate};
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::{Icon, font_paths};
use jackdaw_feathers::tokens;

use super::TerrainEditMode;
use super::ops::{
    TerrainToolFlattenOp, TerrainToolLowerOp, TerrainToolNavmeshOp, TerrainToolNoiseOp,
    TerrainToolQuantizeOp, TerrainToolRaiseOp, TerrainToolSmoothOp,
};
use super::paint::TerrainToolPaintOp;
use super::regions::TerrainToolRegionsOp;
use super::ui_fields::TerrainDefaultFontRoot;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        toggle_palette_visibility.run_if(in_state(crate::AppState::Editor)),
    )
    .add_observer(update_terrain_palette_highlights);
}

const ICON_PX: f32 = 16.0;

/// Edge of one palette button: `FeathersToolButton`'s minimum width, which is
/// what a single-glyph caption leaves it at.
const BUTTON_PX: f32 = 24.0;

/// Width of the palette column, pinned rather than left to the buttons so the
/// gutter the options bar keeps clear is the same number.
const PALETTE_WIDTH_PX: f32 = BUTTON_PX + 2.0 * tokens::SPACING_SM;

/// What the options bar leaves clear on its left: the palette's inset, its
/// width, and a gap between the two.
pub(super) const PALETTE_GUTTER_PX: f32 =
    tokens::SPACING_MD + PALETTE_WIDTH_PX + tokens::SPACING_MD;

/// Distance from the top of the viewport panel's content column to the palette:
/// past the main toolbar and its borders, past one row of the options bar, and
/// then the usual inset.
///
/// Constant, so a taller bar grows downward beside the palette rather than
/// pushing it.
const PALETTE_TOP_PX: f32 =
    tokens::TOOLBAR_HEIGHT + 2.0 + super::options_bar::ROW_HEIGHT_PX + tokens::SPACING_MD;

/// Marker for the terrain tool palette overlay node.
#[derive(Component)]
pub struct TerrainPalette;

/// Builds the terrain tool palette as a `bsn!` Scene. Starts hidden
/// (`Display::None`); shown only while a Terrain entity is selected (see
/// `toggle_palette_visibility`).
///
/// Spawned standalone via `spawn_scene` as a child of the viewport panel's
/// content column; see `viewport::build_viewport_panel`. Each button is a
/// `FeathersToolButton` carrying a `ButtonOperatorCall`: the editor's
/// operator-button glue dispatches the op on `Activate` and greys it out when
/// unavailable, which is how an entry with no terrain selected reads as
/// disabled.
pub fn terrain_palette() -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            left: px(tokens::SPACING_MD),
            top: px(PALETTE_TOP_PX),
            width: px(PALETTE_WIDTH_PX),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(px(tokens::SPACING_SM)),
            row_gap: px(tokens::SPACING_SM),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            display: Display::None,
        }
        BackgroundColor({tokens::TOOLBAR_BG.with_alpha(0.92)})
        TerrainDefaultFontRoot
        Children [
            palette_button(TerrainToolRaiseOp::ID, Icon::Mountain),
            palette_button(TerrainToolLowerOp::ID, Icon::MoveDown),
            palette_button(TerrainToolFlattenOp::ID, Icon::Minus),
            palette_button(TerrainToolSmoothOp::ID, Icon::Waves),
            palette_button(TerrainToolNoiseOp::ID, Icon::Sparkles),
            palette_button(TerrainToolPaintOp::ID, Icon::Paintbrush),
            palette_button(TerrainToolQuantizeOp::ID, Icon::Grid3x3),
            palette_button(TerrainToolNavmeshOp::ID, Icon::Waypoints),
            palette_button(TerrainToolRegionsOp::ID, Icon::Grid2x2),
        ]
    }
}

fn palette_button(op_id: &'static str, icon: Icon) -> impl Scene {
    let glyph = String::from(icon.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! {
                Text(glyph)
                TextFont {
                    font: FontSourceTemplate::Handle(font_paths::LUCIDE),
                    font_size: FontSize::Px(ICON_PX),
                }
            },
            @variant: {ButtonVariant::Plain}
        }
        ButtonOperatorCall::new(op_id)
    }
}

fn toggle_palette_visibility(
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    mut palette: Query<&mut Node, With<TerrainPalette>>,
    mut edit_mode: ResMut<TerrainEditMode>,
) {
    if !selection.is_changed() {
        return;
    }

    let should_show = selection.primary().is_some_and(|e| terrains.contains(e));

    for mut node in &mut palette {
        node.display = if should_show {
            Display::Flex
        } else {
            Display::None
        };
    }

    if !should_show && *edit_mode != TerrainEditMode::None {
        *edit_mode = TerrainEditMode::None;
    }
}

/// Highlights the active palette entry by flipping its [`ButtonVariant`] to
/// `Primary`. Entries are identified by operator id, ids this palette does not
/// own are skipped, and it reruns off [`RefreshOperatorButtons`] so a
/// freshly-spawned palette seeds correctly.
fn update_terrain_palette_highlights(
    _: On<RefreshOperatorButtons>,
    edit_mode: Res<TerrainEditMode>,
    mut buttons: Query<(&ButtonOperatorCall, &mut ButtonVariant)>,
) {
    for (call, mut variant) in &mut buttons {
        let Some(active) = palette_entry_active(&edit_mode, call.id.as_ref()) else {
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

/// Maps a palette operator id to whether its entry is the active mode.
///
/// `None` for any id this palette does not own, which the highlighter leaves
/// alone. `terrain.tool.exit_to_select` (Escape, see `ops.rs`) exits to no-tool
/// but has no button, so it has no entry here.
fn palette_entry_active(mode: &TerrainEditMode, op_id: &str) -> Option<bool> {
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
    } else if op_id == TerrainToolQuantizeOp::ID {
        Some(*mode == TerrainEditMode::Quantize)
    } else if op_id == TerrainToolRegionsOp::ID {
        Some(*mode == TerrainEditMode::Regions)
    } else if op_id == TerrainToolNavmeshOp::ID {
        Some(*mode == TerrainEditMode::Navmesh)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each sculpt sub-tool highlights only its own palette entry.
    #[test]
    fn each_sculpt_tool_highlights_only_its_own_entry() {
        use jackdaw_terrain::SculptTool;

        let ops = [
            (TerrainToolRaiseOp::ID, SculptTool::Raise),
            (TerrainToolLowerOp::ID, SculptTool::Lower),
            (TerrainToolFlattenOp::ID, SculptTool::Flatten),
            (TerrainToolSmoothOp::ID, SculptTool::Smooth),
            (TerrainToolNoiseOp::ID, SculptTool::Noise),
        ];
        for (active_op, active_tool) in ops {
            let mode = TerrainEditMode::Sculpt(active_tool);
            for (op, _) in ops {
                let expected = op == active_op;
                assert_eq!(
                    palette_entry_active(&mode, op),
                    Some(expected),
                    "tool {active_op} active, checked against {op}",
                );
            }
        }
    }

    #[test]
    fn paint_and_quantize_highlight_only_their_own_mode() {
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Paint, TerrainToolPaintOp::ID),
            Some(true)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Quantize, TerrainToolPaintOp::ID),
            Some(false)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Quantize, TerrainToolQuantizeOp::ID),
            Some(true)
        );
    }

    /// The Regions entry highlights on its own mode and on no other.
    #[test]
    fn the_regions_entry_highlights_only_its_own_mode() {
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Regions, TerrainToolRegionsOp::ID),
            Some(true)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Paint, TerrainToolRegionsOp::ID),
            Some(false)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Regions, TerrainToolPaintOp::ID),
            Some(false)
        );
    }

    #[test]
    fn the_navmesh_entry_highlights_only_in_its_own_mode() {
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Navmesh, TerrainToolNavmeshOp::ID),
            Some(true)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::None, TerrainToolNavmeshOp::ID),
            Some(false)
        );
        assert_eq!(
            palette_entry_active(&TerrainEditMode::Quantize, TerrainToolNavmeshOp::ID),
            Some(false)
        );
    }

    #[test]
    fn unrelated_operator_ids_are_left_alone() {
        assert_eq!(
            palette_entry_active(&TerrainEditMode::None, "some.other.operator"),
            None
        );
    }
}
