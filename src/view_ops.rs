//! View-mode toggles: wireframe, bounding boxes, face grid, etc.
//!
//! Each op just flips a resource. `allows_undo = false` so they don't
//! push snapshot diffs to the history stack. Only `view.toggle_wireframe`
//! has a default keybind (`Ctrl+Shift+W`); the rest are menu-only.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Chord, Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::{CoreExtensionInputContext, Modifiers};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext, modifiers: &Modifiers) {
    ctx.register_operator::<ViewToggleWireframeOp>()
        .register_operator::<ViewToggleBoundingBoxesOp>()
        .register_operator::<ViewCycleBoundingBoxModeOp>()
        .register_operator::<ViewToggleFaceGridOp>()
        .register_operator::<ViewToggleBrushWireframeOp>()
        .register_operator::<ViewToggleBrushOutlineOp>()
        .register_operator::<ViewToggleAlignmentGuidesOp>()
        .register_operator::<ViewToggleColliderGizmosOp>()
        .register_operator::<ViewToggleHierarchyArrowsOp>();

    let ext = ctx.id();
    let ctrl = modifiers.ctrl;
    let shift = modifiers.shift;
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<ViewToggleWireframeOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            Chord::new([ctrl, shift]),
            bindings![(KeyCode::KeyW, Press::default())],
        ));
    });
}

#[operator(
    id = "view.toggle_wireframe",
    label = "Toggle Wireframe",
    allows_undo = false
)]
fn view_toggle_wireframe(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::view_modes::ViewModeSettings>,
) -> OperatorResult {
    settings.wireframe = !settings.wireframe;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_bounding_boxes",
    label = "Toggle Bounding Boxes",
    allows_undo = false
)]
fn view_toggle_bounding_boxes(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_bounding_boxes = !settings.show_bounding_boxes;
    OperatorResult::Finished
}

#[operator(
    id = "view.cycle_bounding_box_mode",
    label = "Cycle Bounding Box Mode",
    allows_undo = false
)]
fn view_cycle_bounding_box_mode(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.bounding_box_mode = match settings.bounding_box_mode {
        crate::viewport_overlays::BoundingBoxMode::Aabb => {
            crate::viewport_overlays::BoundingBoxMode::ConvexHull
        }
        crate::viewport_overlays::BoundingBoxMode::ConvexHull => {
            crate::viewport_overlays::BoundingBoxMode::Aabb
        }
    };
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_face_grid",
    label = "Toggle Face Grid",
    allows_undo = false
)]
fn view_toggle_face_grid(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_face_grid = !settings.show_face_grid;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_brush_wireframe",
    label = "Toggle Brush Wireframe",
    allows_undo = false
)]
fn view_toggle_brush_wireframe(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_brush_wireframe = !settings.show_brush_wireframe;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_brush_outline",
    label = "Toggle Brush Outline",
    allows_undo = false
)]
fn view_toggle_brush_outline(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_brush_outline = !settings.show_brush_outline;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_alignment_guides",
    label = "Toggle Alignment Guides",
    allows_undo = false
)]
fn view_toggle_alignment_guides(
    _: In<OperatorParameters>,
    mut settings: ResMut<crate::viewport_overlays::OverlaySettings>,
) -> OperatorResult {
    settings.show_alignment_guides = !settings.show_alignment_guides;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_collider_gizmos",
    label = "Toggle Collider Gizmos",
    allows_undo = false
)]
fn view_toggle_collider_gizmos(
    _: In<OperatorParameters>,
    mut config: ResMut<jackdaw_avian_integration::PhysicsOverlayConfig>,
) -> OperatorResult {
    config.show_colliders = !config.show_colliders;
    OperatorResult::Finished
}

#[operator(
    id = "view.toggle_hierarchy_arrows",
    label = "Toggle Hierarchy Arrows",
    allows_undo = false
)]
fn view_toggle_hierarchy_arrows(
    _: In<OperatorParameters>,
    mut config: ResMut<jackdaw_avian_integration::PhysicsOverlayConfig>,
) -> OperatorResult {
    config.show_hierarchy_arrows = !config.show_hierarchy_arrows;
    OperatorResult::Finished
}
