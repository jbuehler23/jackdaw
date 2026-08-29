//! Stage selection and manipulation: clicking the 2D viewport picks the
//! authored UI node under the cursor and outlines it, and dragging the
//! outline edits it.
//!
//! Five contracts are pinned here:
//!
//! 1. The stage-to-authored mapping is the reference-resolution scale and
//!    nothing else: a round trip between the render-target pixels
//!    `Ui2dView` is stated in and the authored pixels the scene lays out
//!    in.
//! 2. Overlapping authored rects resolve the way Bevy paints them:
//!    `ComputedStackIndex` first, tree order second.
//! 3. A click on the stage selects that node, spawns exactly one outline
//!    with eight handles over its live rect, and never reaches the dock.
//! 4. A gesture is one history entry, undoing to the exact `Node` it
//!    started from; a gesture that moved nothing, or that Escape
//!    abandoned, is no entry at all.
//! 5. Snapping pulls a dragged edge onto a neighbouring one, and Ctrl
//!    inverts that for the length of the gesture.

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::{
        backend::HitData,
        events::{Drag, DragEnd, DragStart, Pointer, Press},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw_scene_types::{CanvasGuides, UiSceneRoot};
use jackdaw_snap::SnapLine;

use jackdaw::{
    canvas_snap::{CanvasSnap, CanvasSnapKind},
    selection::Selection,
    ui_stage::{
        CandidateKind, CanvasAxis, ExactPercent, GuideLine, HANDLE_SIZE, NodeAnchors,
        PixelRounding, SnapHighlight, SnapOutcome, StageHit, UiManipulation, UiResizeHandle,
        UiSelectionOverlay, UnitBasis, apply_authored_rect, authored_to_stage,
        stage_pixels_per_target_pixel, stage_to_authored, topmost_hit,
    },
    viewport_2d::{
        CanvasRuler, RULER_SIZE, RulerGuideMark, Ui2dView, Viewport2dMode, Viewport2dPanelHost,
        build_viewport_2d_panel, ruler_marks, target_pixels_per_stage_pixel,
    },
};

mod util;

use jackdaw_api::op::OperatorWorldExt as _;
use util::OperatorResultExt as _;

/// The reference resolution used by the world tests: twice the stage the
/// panel below lays out, so every conversion factor is an exact 2.
const REFERENCE: UVec2 = UVec2::new(2400, 1200);

#[test]
fn the_stage_and_the_authored_canvas_round_trip() {
    let target = UVec2::new(1280, 720);

    // The centre of the stage is the centre of the authored canvas.
    assert_eq!(
        stage_to_authored(Vec2::ZERO, target),
        Vec2::new(640.0, 360.0),
    );
    // ... and the authored origin is the stage's top-left corner.
    assert_eq!(
        authored_to_stage(Vec2::ZERO, target),
        Vec2::new(-640.0, -360.0),
    );

    for offset in [
        Vec2::ZERO,
        Vec2::new(120.0, -45.0),
        Vec2::new(-640.0, 360.0),
    ] {
        let round_tripped = authored_to_stage(stage_to_authored(offset, target), target);
        assert!(
            (round_tripped - offset).length() < 1e-3,
            "{offset:?} round-tripped to {round_tripped:?}",
        );
    }
}

/// The overlay is drawn in the stage's own logical pixels, so an authored
/// measurement takes both factors back out: the reference-resolution scale
/// and the UI scale factor.
#[test]
fn authored_pixels_scale_into_the_stage_node_through_both_factors() {
    // A 2400-wide reference shown in a 1200-wide stage: two authored
    // pixels per stage physical pixel. At a UI scale factor of 2 that
    // stage is 600 logical px wide, so an authored pixel is a quarter of
    // a logical one.
    let scale = target_pixels_per_stage_pixel(Vec2::new(1200.0, 600.0), REFERENCE);
    assert_eq!(scale, 2.0);
    assert_eq!(stage_pixels_per_target_pixel(scale, 0.5), 0.25);
    // With no UI scale in play the reference scale is the only factor.
    assert_eq!(stage_pixels_per_target_pixel(scale, 1.0), 0.5);

    // A degenerate stage must not divide by zero: this runs every frame a
    // panel is selected, including the one it is first laid out on.
    assert_eq!(stage_pixels_per_target_pixel(0.0, 1.0), 1.0);
}

/// Paint order is `ComputedStackIndex`: assigned by `ui_stack_system`
/// from the tree walk and `ZIndex` together, unique per node, so it
/// decides on its own wherever layout has run. The click has to agree
/// with what is drawn.
#[test]
fn the_topmost_hit_is_the_last_painted_rect() {
    let a = Entity::from_raw_u32(1).unwrap();
    let b = Entity::from_raw_u32(2).unwrap();
    let c = Entity::from_raw_u32(3).unwrap();

    let hit = |entity, min: Vec2, max: Vec2, stack| StageHit {
        entity,
        rect: Rect::from_corners(min, max),
        stack,
    };

    // Overlapping siblings as a stack pass leaves them: the later one is
    // painted over the earlier, and carries the higher index to say so.
    let siblings = [
        hit(a, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0), 3),
        hit(b, Vec2::new(50.0, 50.0), Vec2::new(150.0, 150.0), 4),
    ];
    assert_eq!(topmost_hit(Vec2::new(75.0, 75.0), &siblings), Some(b));
    assert_eq!(topmost_hit(Vec2::new(25.0, 25.0), &siblings), Some(a));
    assert_eq!(topmost_hit(Vec2::new(400.0, 25.0), &siblings), None);

    // A `ZIndex` that lifts the earlier sibling shows up as the higher
    // index, and outranks tree order.
    let raised = [
        hit(a, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0), 9),
        hit(b, Vec2::new(50.0, 50.0), Vec2::new(150.0, 150.0), 3),
    ];
    assert_eq!(topmost_hit(Vec2::new(75.0, 75.0), &raised), Some(a));

    // A child inside its parent, likewise: later in the walk, higher
    // index. The ordinary case of clicking a button inside a panel.
    let nested = [
        hit(a, Vec2::new(0.0, 0.0), Vec2::new(200.0, 200.0), 1),
        hit(c, Vec2::new(20.0, 20.0), Vec2::new(60.0, 60.0), 2),
    ];
    assert_eq!(topmost_hit(Vec2::new(40.0, 40.0), &nested), Some(c));
    assert_eq!(topmost_hit(Vec2::new(150.0, 150.0), &nested), Some(a));

    // Before the first stack pass every node reads 0, and the tiebreak
    // falls back to the depth-first order the hits were collected in,
    // which matches what Bevy paints last once the pass runs.
    let unstacked = [
        hit(a, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0), 0),
        hit(b, Vec2::new(50.0, 50.0), Vec2::new(150.0, 150.0), 0),
    ];
    assert_eq!(topmost_hit(Vec2::new(75.0, 75.0), &unstacked), Some(b));
}

/// The whole path, against real layout: a click lands on the authored node
/// under it, and the outline follows that node's live rect.
#[test]
fn clicking_the_stage_selects_and_outlines_the_authored_node() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, back, front) = authored_scene(&mut app);
    settle(&mut app);

    // The overlap of the two children: `front` is the later sibling, so
    // it is what is drawn there.
    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);

    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![front],
        "the later sibling is painted on top, so it is what the click picks",
    );

    let (overlay, node) = overlay_node(&mut app);
    assert_eq!(
        app.world().get::<ChildOf>(overlay).map(ChildOf::parent),
        Some(stage_entity(&mut app, panel)),
        "the overlay is editor chrome parented into the stage, never into the authored tree",
    );
    assert!(
        app.world().get::<jackdaw::EditorEntity>(overlay).is_some(),
        "the overlay must be marked as editor chrome",
    );

    // `front` is authored at (400, 200) 400x200 in a 2400-wide reference
    // shown in a 1200-wide stage, so the outline sits at half of that.
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(200), px(100), px(200), px(100)),
        "the outline covers the authored rect scaled into the stage",
    );

    let handles = handle_layout(&mut app, overlay);
    assert_eq!(handles.len(), 8, "eight handles, one per edge and corner");
    for corner in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
    ] {
        assert!(
            handles.contains(&corner),
            "missing the {corner:?} handle in {handles:?}",
        );
    }
    assert_eq!(HANDLE_SIZE, 8.0);

    // Clicking the part of `back` that `front` does not cover moves the
    // selection, and the single overlay follows rather than doubling up.
    click_authored(&mut app, panel, Vec2::new(250.0, 150.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![back]);
    let (_, node) = overlay_node(&mut app);
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(100), px(50), px(200), px(100)),
        "the outline moved to the newly selected node",
    );

    // Five authored pixels either side of `front`'s left edge, over ground
    // both children cover: the click resolves to the exact edge, not to
    // the neighbourhood of it.
    click_authored(&mut app, panel, Vec2::new(405.0, 250.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![front]);
    click_authored(&mut app, panel, Vec2::new(395.0, 250.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![back]);

    // The same on the other axis, across `front`'s bottom edge.
    click_authored(&mut app, panel, Vec2::new(700.0, 395.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![front]);
    click_authored(&mut app, panel, Vec2::new(700.0, 405.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![root]);

    // The root fills the canvas, so a click that misses both children
    // still lands on something authored rather than clearing.
    click_authored(&mut app, panel, Vec2::new(1800.0, 900.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![root]);
}

/// The outline covers the whole selected node, so a press anywhere on a
/// selected container lands on the overlay rather than on the stage.
/// Claiming those as the move gesture without asking what is under them
/// makes a selected container swallow every click on its children.
///
/// So an overlay press re-resolves. Anything that is not the selection,
/// a child or an overlapping sibling, is selected instead. That is a
/// selection and not an edit: no gesture runs and no history entry is
/// pushed.
#[test]
fn a_press_on_a_selected_containers_child_selects_the_child() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, front) = authored_scene(&mut app);
    settle(&mut app);

    // The root fills the canvas, so its outline is over every child.
    select(&mut app, root);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let entries = history_len(&app);
    let before = node_of(&app, front);

    press_at(&mut app, panel, Vec2::new(500.0, 250.0), overlay);
    settle(&mut app);

    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![front],
        "a press on the overlay over a child selects the child",
    );
    assert_eq!(
        node_of(&app, front),
        before,
        "selecting is not moving: nothing was dragged",
    );
    assert_eq!(
        history_len(&app),
        entries,
        "and a selection is not an undoable edit",
    );
}

/// The other half of the rule: a press on the selected node's *own* body
/// is the move gesture.
#[test]
fn a_press_on_the_selected_nodes_own_body_still_moves_it() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let entries = history_len(&app);

    press_at(&mut app, panel, Vec2::new(500.0, 250.0), overlay);
    settle(&mut app);
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![front],
        "the press claims the gesture rather than re-selecting under it",
    );

    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(500.0, 250.0),
        Vec2::new(560.0, 280.0),
    );
    settle(&mut app);

    let moved = node_of(&app, front);
    assert_eq!(
        (moved.left, moved.top),
        (px(460), px(230)),
        "the move runs exactly as it did before",
    );
    assert_eq!(history_len(&app), entries + 1, "and is one history entry");
}

/// Selection chrome is drawn over content the editor does not control,
/// so a single accent colour would disappear against content of the same
/// luminance.
///
/// The handles are a light neutral fill with an accent border, and the
/// outline carries a dark edge just outside its accent line. Between them
/// the chrome has a light part and a dark part on every side, and no
/// single content colour can swallow both.
#[test]
fn the_selection_chrome_reads_against_content_of_any_colour() {
    use bevy::picking::prelude::Pickable;
    use jackdaw_feathers::tokens;

    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);

    assert_eq!(
        app.world()
            .get::<BorderColor>(overlay)
            .map(|border| border.top),
        Some(tokens::ACCENT_BLUE),
        "the outline itself stays the accent line the user knows",
    );

    let children: Vec<Entity> = app
        .world()
        .get::<Children>(overlay)
        .map(|children| children.iter().collect())
        .unwrap_or_default();

    let handles: Vec<Entity> = children
        .iter()
        .copied()
        .filter(|child| app.world().get::<UiResizeHandle>(*child).is_some())
        .collect();
    assert_eq!(handles.len(), 8);
    for handle in handles {
        assert_eq!(
            app.world().get::<BackgroundColor>(handle).map(|bg| bg.0),
            Some(tokens::TEXT_PRIMARY),
            "a handle reads on dark content by its light fill",
        );
        assert_eq!(
            app.world()
                .get::<BorderColor>(handle)
                .map(|border| border.top),
            Some(tokens::ACCENT_BLUE),
            "... and on light content by its accent border",
        );
        let node = node_of(&app, handle);
        assert_eq!(
            (node.width, node.height),
            (px(HANDLE_SIZE), px(HANDLE_SIZE)),
            "the border is inside the square, so the target is what is drawn",
        );
    }

    // One node for the dark edge, not one per side, and it must not take
    // the press that belongs to the outline body underneath it.
    let edges: Vec<Entity> = children
        .iter()
        .copied()
        .filter(|child| {
            app.world().get::<UiResizeHandle>(*child).is_none()
                && app.world().get::<BorderColor>(*child).is_some()
        })
        .collect();
    assert_eq!(edges.len(), 1, "one node draws the whole dark edge");
    assert_eq!(
        app.world()
            .get::<BorderColor>(edges[0])
            .map(|border| border.top),
        Some(tokens::SHADOW_COLOR),
    );
    assert_eq!(
        app.world().get::<Pickable>(edges[0]).copied(),
        Some(Pickable::IGNORE),
        "a dark edge that ate the press would undo the outline re-resolve",
    );
}

/// A press that starts a selection must not climb into the dock, or a
/// gesture started on the stage becomes a panel drag.
#[test]
fn a_stage_press_never_reaches_the_dock() {
    #[derive(Resource, Default)]
    struct DockPresses(usize);

    let mut app = stage_app();
    app.init_resource::<DockPresses>();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    settle(&mut app);

    app.world_mut().entity_mut(panel).observe(
        |_: On<Pointer<Press>>, mut presses: ResMut<DockPresses>| {
            presses.0 += 1;
        },
    );

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);

    assert_eq!(
        app.world().resource::<DockPresses>().0,
        0,
        "the stage press must be stopped before it propagates into the panel",
    );
}

/// A selected node with no layout this frame, hidden or rebuilt and not
/// yet measured, holds the outline where it is instead of dropping it.
/// An overlay that vanishes for a frame takes any gesture running on it
/// with it.
#[test]
fn an_unmeasured_node_holds_the_outline_rather_than_dropping_it() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, placed) = overlay_node(&mut app);

    // Collapse the selected node's layout without deselecting it.
    if let Some(mut node) = app.world_mut().get_mut::<Node>(front) {
        node.display = Display::None;
    }
    settle(&mut app);

    let (still_there, held) = overlay_node(&mut app);
    assert_eq!(still_there, overlay, "the same overlay entity survives");
    assert_eq!(
        (held.left, held.top, held.width, held.height),
        (placed.left, placed.top, placed.width, placed.height),
        "the outline holds the last rect it had",
    );

    // Deselecting is what drops it.
    app.world_mut().resource_mut::<Selection>().entities.clear();
    settle(&mut app);
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<UiSelectionOverlay>>()
            .iter(app.world())
            .count(),
        0,
        "an empty selection drops the overlay",
    );
}

/// The path a real click takes. The stage's frame child covers it
/// completely, so picking never targets the stage itself: the press lands
/// on the frame and reaches the observer by propagating up. A selection
/// that only resolved an event aimed at the stage directly would pass
/// every other test here and do nothing in the editor.
#[test]
fn a_press_on_the_frame_child_selects_through_propagation() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    let stage = stage_entity(&mut app, panel);
    let frame = app
        .world()
        .get::<Children>(stage)
        .and_then(|children| children.iter().next())
        .expect("the stage has its frame child");
    assert_ne!(frame, stage);

    press_at(&mut app, panel, Vec2::new(500.0, 250.0), frame);
    settle(&mut app);

    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![front],
        "a press on the frame must reach the stage observer by propagation",
    );
}

/// Pan and zoom live in the stage's placement, so a click still lands on
/// the authored pixel under the cursor after the view moves.
///
/// The click position here is derived from the area and the view, not
/// from the stage node, so it agrees with the production path only if
/// `place_stage` put the canvas where the view says. Dropping the zoom
/// from the placement puts every one of these clicks on the wrong node.
#[test]
fn a_zoomed_and_panned_view_still_selects_what_the_cursor_is_over() {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 2.0);
    let (root, back, front) = authored_scene(&mut app);
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .pan = Vec2::new(500.0, -250.0);
    settle(&mut app);

    // At zoom 2 a stage pixel is half an authored pixel, and the pan has
    // put authored (1700, 850) at the centre of the area.
    let stage = stage_entity(&mut app, panel);
    assert_eq!(
        target_pixels_per_stage_pixel(
            app.world()
                .get::<ComputedNode>(stage)
                .expect("the stage is laid out")
                .size(),
            REFERENCE,
        ),
        0.5,
        "the stage carries the zoom, which is how the cursor math finds it",
    );

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![front]);

    // The same +-5 authored pixel probes as at the default view: a zoom
    // the mapping had not accounted for would miss by far more.
    click_authored(&mut app, panel, Vec2::new(395.0, 250.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![back]);

    click_authored(&mut app, panel, Vec2::new(700.0, 405.0));
    settle(&mut app);
    assert_eq!(app.world().resource::<Selection>().entities, vec![root]);

    // The overlay follows the zoom because it is a child of the stage
    // that carries it: `front` is 400x200 authored, so 800x400 of stage
    // at zoom 2.
    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (_, node) = overlay_node(&mut app);
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(800), px(400), px(800), px(400)),
        "the outline is placed and scaled by the stage it hangs in",
    );
}

/// Panning the view does not move the authored scene, so the stage-to-
/// authored mapping has no camera term in it.
///
/// Bevy renders UI through its own view (`extract_ui_camera_view` builds an
/// orthographic projection straight from the target's viewport rect and
/// parks the view transform at the origin), so a routed UI scene is pinned
/// to its render target whatever the 2D camera is doing. A hit test that
/// went through `world_at` would drift off the visible pixels by exactly
/// the pan.
#[test]
fn the_view_does_not_move_the_authored_scene_under_the_cursor() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    let before = *app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(front)
        .expect("the authored node is laid out");

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view = Ui2dView {
        pan: Vec2::new(640.0, -320.0),
        zoom: 3.0,
        ..default()
    };
    app.world_mut()
        .run_system_cached(jackdaw::viewport_2d::apply_2d_view)
        .expect("apply_2d_view ran");
    settle(&mut app);

    let after = *app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(front)
        .expect("the authored node is still laid out");
    assert_eq!(
        before.translation, after.translation,
        "a panned camera must not move the authored UI it renders",
    );

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![front],
        "the same authored point is still under the same stage point",
    );
}

/// One drag of the outline is one history entry, and undoing it hands
/// back the exact `Node` the gesture started from: not a rounded
/// approximation, and not an intermediate value from the middle of the
/// drag.
#[test]
fn a_move_gesture_is_one_undoable_entry() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let before = node_of(&app, front);
    let entries = history_len(&app);
    let (overlay, _) = overlay_node(&mut app);

    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(500.0, 250.0),
        Vec2::new(560.0, 280.0),
    );
    settle(&mut app);

    let moved = node_of(&app, front);
    assert_eq!(
        (moved.left, moved.top, moved.width, moved.height),
        (px(460), px(230), px(400), px(200)),
        "a move slides the authored offset by the drag, in authored pixels",
    );
    assert_eq!(
        history_len(&app),
        entries + 1,
        "a whole gesture is one entry, however many drag events it took",
    );

    undo(&mut app);
    settle(&mut app);
    assert_eq!(
        node_of(&app, front),
        before,
        "undo restores the node the gesture started from",
    );
}

/// The top-left handle drags two edges at once: the offset follows the
/// cursor and the size shrinks by the same amount, leaving the opposite
/// corner where it was.
#[test]
fn the_top_left_handle_moves_the_offset_and_the_size_together() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let handle = handle_entity(&mut app, overlay, (-1, -1));

    drag_authored(
        &mut app,
        panel,
        handle,
        Vec2::new(400.0, 200.0),
        Vec2::new(440.0, 220.0),
    );
    settle(&mut app);

    let resized = node_of(&app, front);
    assert_eq!(
        (
            resized.left,
            resized.top,
            resized.width,
            resized.height,
            resized.right,
            resized.bottom,
        ),
        (px(440), px(220), px(360), px(180), Val::Auto, Val::Auto),
        "the dragged corner moves and the far corner stays at (800, 400)",
    );
}

/// The wheel still belongs to the panel while a gesture is running, so a
/// drag keeps tracking the cursor across a zoom mid-drag.
///
/// The conversion from pointer pixels to authored ones is the view's, and
/// a copy of it taken at the press is stale the moment the view moves:
/// the node then trails the cursor at half speed, or outruns it at
/// double, for the rest of the gesture.
#[test]
fn a_zoom_mid_drag_moves_the_drag_onto_the_new_scale() {
    let mut app = stage_app();
    // Half zoom: two authored pixels per pointer pixel.
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let start = begin_drag(&mut app, panel, overlay, Vec2::new(500.0, 250.0));

    // The canvas is zoomed to 1:1 with the gesture still down.
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .zoom = 1.0;
    settle(&mut app);

    continue_drag(&mut app, overlay, start, Vec2::new(100.0, 0.0));
    end_drag(&mut app, overlay, start, Vec2::new(100.0, 0.0));
    settle(&mut app);

    assert_eq!(
        node_of(&app, front).left,
        px(500),
        "at 1:1 a hundred pointer pixels are a hundred authored ones",
    );
}

/// A handle dragged past the edge opposite it stops there. The size
/// bottoms out at a pixel, and the origin bottoms out with it: an origin
/// that kept following the cursor would walk the node off across the
/// canvas one pixel wide.
#[test]
fn a_resize_dragged_past_the_far_edge_stops_at_it() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let handle = handle_entity(&mut app, overlay, (-1, -1));

    // `front` spans authored 400..800 by 200..400, so this drags the
    // top-left corner a long way past the bottom-right one.
    drag_authored(
        &mut app,
        panel,
        handle,
        Vec2::new(400.0, 200.0),
        Vec2::new(900.0, 500.0),
    );
    settle(&mut app);

    let resized = node_of(&app, front);
    assert_eq!(
        (resized.left, resized.top, resized.width, resized.height),
        (px(799), px(399), px(1), px(1)),
        "the dragged corner stops a pixel short of the corner it cannot pass",
    );
}

/// A press and release that never moved is a click, not an edit, and
/// leaves no history entry.
#[test]
fn a_click_that_never_moved_records_nothing() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let before = node_of(&app, front);
    let entries = history_len(&app);
    let (overlay, _) = overlay_node(&mut app);

    let start = begin_drag(&mut app, panel, overlay, Vec2::new(500.0, 250.0));
    end_drag(&mut app, overlay, start, Vec2::ZERO);
    settle(&mut app);

    assert_eq!(
        node_of(&app, front),
        before,
        "nothing moved, nothing changed"
    );
    assert_eq!(
        history_len(&app),
        entries,
        "a no-op gesture records nothing"
    );
}

/// Escape abandons the gesture: the node goes back to exactly what it
/// was, and the history never hears about it.
#[test]
fn escape_abandons_a_gesture_in_progress() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let before = node_of(&app, front);
    let entries = history_len(&app);
    let (overlay, _) = overlay_node(&mut app);

    let start = begin_drag(&mut app, panel, overlay, Vec2::new(500.0, 250.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(560.0, 280.0)) - start;
    continue_drag(&mut app, overlay, start, distance);
    settle(&mut app);
    assert_eq!(
        node_of(&app, front).left,
        px(460),
        "the drag is live before the cancel",
    );

    press_escape(&mut app);
    settle(&mut app);

    assert_eq!(
        node_of(&app, front),
        before,
        "escape restores the node the gesture started from",
    );
    assert_eq!(
        history_len(&app),
        entries,
        "an abandoned gesture records nothing",
    );
}

/// With snapping on, a move lands on a sibling's edge rather than where
/// the cursor stopped; Ctrl inverts that for the gesture, as in the 3D
/// tools.
#[test]
fn a_snapped_move_lands_on_a_sibling_edge_unless_ctrl_says_otherwise() {
    // `back` spans authored x 200..600, so its right edge is a candidate
    // four pixels from where this drag would otherwise leave `front`.
    let dragged = |ctrl: bool| {
        let mut app = stage_app();
        let panel = panel_entity(&mut app);
        let (_, _, front) = authored_scene(&mut app);
        settle(&mut app);

        click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
        settle(&mut app);
        if ctrl {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ControlLeft);
        }
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(500.0, 250.0),
            Vec2::new(704.0, 250.0),
        );
        settle(&mut app);
        node_of(&app, front).left
    };

    assert_eq!(
        dragged(false),
        px(600),
        "the near sibling edge pulls the move onto it",
    );
    assert_eq!(
        dragged(true),
        px(604),
        "ctrl inverts snapping, so the move lands where the cursor did",
    );
}

/// The canvas has a grid of its own, in authored pixels, and a snapped
/// gesture that no neighbour claimed lands on it.
///
/// The 3D grid is a lattice of world units: at the editor's default
/// power it rounds an authored pixel to a quarter of one, and at a large
/// power it pins a button to a lattice measured in metres. The stage
/// carries its own pixel grid instead, and both the offset a move writes
/// and the size a resize writes land on it.
#[test]
fn a_snapped_gesture_lands_on_the_canvas_pixel_grid() {
    assert_eq!(
        jackdaw::viewport_2d::DEFAULT_UI_GRID,
        8.0,
        "the default lattice is eight authored pixels",
    );

    // 61 by 37 authored pixels: clear of every sibling and parent edge
    // the gesture could otherwise land on, so the grid is the only thing
    // that can move it.
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(500.0, 250.0),
        Vec2::new(561.0, 287.0),
    );
    settle(&mut app);

    let moved = node_of(&app, front);
    assert_eq!(
        (moved.left, moved.top),
        (px(464), px(240)),
        "461 by 237 rounds onto the eight-pixel lattice",
    );

    // The same lattice under a resize: the dragged corner lands on it,
    // and the opposite one stays where it was, so the size follows.
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let handle = handle_entity(&mut app, overlay, (1, 1));
    drag_authored(
        &mut app,
        panel,
        handle,
        Vec2::new(800.0, 400.0),
        Vec2::new(861.0, 437.0),
    );
    settle(&mut app);

    let resized = node_of(&app, front);
    assert_eq!(
        (resized.width, resized.height),
        (px(464), px(240)),
        "the dragged corner lands on the lattice at 864 by 440",
    );
}

/// Snapping is one switch. Ctrl inverts the snap toggle for the length
/// of a gesture, and what it inverts is the grid and the neighbouring
/// edges together.
///
/// Consulting the raw `translate_snap` field a second time, as
/// `snap_translate_vec2` does, would make Ctrl mean "edges only", so the
/// toggle's off state would mean one thing next to a sibling and another
/// out in the open.
#[test]
fn ctrl_turns_the_grid_and_the_edges_on_together() {
    // The master is off throughout: Ctrl is the whole switch here.
    let landed = |to: Vec2| {
        let mut app = stage_app();
        let panel = panel_entity(&mut app);
        let (_, _, front) = authored_scene(&mut app);
        magnet(&mut app, false);
        settle(&mut app);

        click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
        settle(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ControlLeft);
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(&mut app, panel, overlay, Vec2::new(500.0, 250.0), to);
        settle(&mut app);
        let node = node_of(&app, front);
        (node.left, node.top)
    };

    assert_eq!(
        landed(Vec2::new(561.0, 287.0)),
        (px(464), px(240)),
        "ctrl turns the grid on, out in the open",
    );
    assert_eq!(
        landed(Vec2::new(704.0, 250.0)),
        (px(600), px(200)),
        "... and the same ctrl lands the same gesture on a sibling edge",
    );
}

/// A canvas that has had nothing switched on snaps.
///
/// The master is the canvas's own, and it ships on, so the first drag a
/// new user makes lands on its neighbour. Shipping it off would leave
/// the rulers, the guides and every kind in the menu doing nothing until
/// a switch nothing points at had been found and turned.
#[test]
fn a_drag_on_a_default_canvas_already_snaps() {
    let mut app = stage_app();
    assert!(
        app.world().resource::<CanvasSnap>().enabled,
        "the canvas's magnet is on out of the box",
    );
    let panel = panel_entity(&mut app);
    let (_, _, front) = authored_scene(&mut app);
    settle(&mut app);

    click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(500.0, 250.0),
        Vec2::new(704.0, 250.0),
    );
    settle(&mut app);

    assert_eq!(
        node_of(&app, front).left,
        px(600),
        "the sibling's edge pulled the drag onto it, with nothing turned on first",
    );
}

/// The snap radius is a distance on screen, not in the canvas: a cursor
/// stopping a given number of *screen* pixels past a neighbour snaps, or
/// does not, identically at any zoom.
///
/// A radius stated in authored pixels passes the single-zoom tests above
/// and then shrinks and grows with the zoom.
#[test]
fn the_snap_radius_is_the_same_on_screen_at_any_zoom() {
    // `back`'s right edge is authored 600 and `front` starts at 400, so
    // a drag of 200 authored pixels puts the two edges together; `over`
    // is how far past that the cursor stops, in screen pixels.
    let landed = |zoom: f32, over: f32| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, zoom);
        let (_, _, front) = authored_scene(&mut app);
        without_the_pixel_grid(&mut app, panel);
        settle(&mut app);

        click_authored(&mut app, panel, Vec2::new(500.0, 250.0));
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        // One screen pixel is `1 / zoom` authored pixels.
        let target = 500.0 + 200.0 + over / zoom;
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(500.0, 250.0),
            Vec2::new(target, 250.0),
        );
        settle(&mut app);
        node_of(&app, front).left
    };

    // Four screen pixels past the edge: inside the radius at both zooms,
    // even though that is eight authored pixels at one and two at the
    // other.
    assert_eq!(
        landed(0.5, 4.0),
        px(600),
        "zoomed out, four screen px snaps"
    );
    assert_eq!(landed(2.0, 4.0), px(600), "zoomed in, four screen px snaps");

    // Eight screen pixels: outside it at both, so each lands where the
    // cursor left it.
    assert_eq!(
        landed(0.5, 8.0),
        px(616),
        "zoomed out, eight screen px is past the radius",
    );
    assert_eq!(
        landed(2.0, 8.0),
        px(604),
        "zoomed in, eight screen px is past the radius",
    );
}

/// A child's `left`/`top` are measured from inside its parent's border,
/// so everything the gesture compares them against has to be too.
///
/// The two shifts a border introduces do not cancel: the offset a
/// promoted flex child starts from is read out of layout, and the value
/// written back to `Node::left` is read by Bevy as padding-relative.
#[test]
fn offsets_inside_a_bordered_parent_are_measured_from_the_padding_box() {
    // A drag that does not snap: the promoted child lands where the
    // cursor took it, measured from inside the border.
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let flexed = bordered_scene(&mut app);
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    settle(&mut app);

    select(&mut app, flexed);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(260.0, 135.0),
        Vec2::new(290.0, 150.0),
    );
    settle(&mut app);

    let promoted = node_of(&app, flexed);
    assert_eq!(
        (promoted.position_type, promoted.left, promoted.top),
        (PositionType::Absolute, px(30), px(15)),
        "promoting a flex child moves it by the drag and not by the border",
    );

    // A snapped drag lands on the sibling's edge itself, not a
    // border-width past it.
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let flexed = bordered_scene(&mut app);
    app.world_mut()
        .resource_mut::<jackdaw::snapping::SnapSettings>()
        .translate_snap = true;
    settle(&mut app);

    select(&mut app, flexed);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    // The sibling's left edge is 400 from the padding box; stop two
    // authored pixels short of it.
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(260.0, 135.0),
        Vec2::new(260.0 + 398.0, 135.0),
    );
    settle(&mut app);

    assert_eq!(
        node_of(&app, flexed).left,
        px(400),
        "the snap lands on the sibling's edge, in the sibling's own space",
    );
}

// ---------------------------------------------------------------------------
// The arrow keys nudge the canvas
// ---------------------------------------------------------------------------

/// The arrows move the whole selection a pixel at a time, and Shift
/// moves it by the canvas grid the header's stepper sets.
#[test]
fn the_arrow_keys_nudge_the_selected_nodes() {
    let (mut app, panel, nodes) = selection_app();
    let [first, _, third] = nodes;
    select_all(&mut app, &nodes);
    settle(&mut app);
    let before: Vec<Node> = nodes.iter().map(|node| node_of(&app, *node)).collect();
    let entries = history_len(&app);

    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    assert_eq!(
        (node_of(&app, first).left, node_of(&app, third).left),
        (px(201), px(1201)),
        "one press is one authored pixel, on every selected node",
    );
    assert_eq!(
        history_len(&app),
        entries + 1,
        "and one press is one entry, whatever it moved",
    );

    // Shift takes the canvas grid instead: the panel's own grid, the one
    // the header's stepper sets.
    set_grid(&mut app, panel, 8.0);
    hold_shift(&mut app);
    nudge(&mut app, "transform.nudge_z_neg");
    settle(&mut app);
    assert_eq!(
        (node_of(&app, first).top, node_of(&app, third).top),
        (px(92), px(692)),
        "Shift and the up arrow move the selection one grid step up the canvas",
    );

    undo(&mut app);
    undo(&mut app);
    settle(&mut app);
    assert_eq!(
        nodes
            .iter()
            .map(|node| node_of(&app, *node))
            .collect::<Vec<_>>(),
        before,
        "and both presses undo, one entry each",
    );
}

/// A container and a node inside it, both selected, move together once.
///
/// Layout already carries the child when its container moves. Writing
/// the gesture's delta to both would move the child twice, once by the
/// container's offset and once by its own, and the child would drift out
/// of the container by the nudge distance.
#[test]
fn a_container_and_its_child_selected_together_move_the_child_once() {
    let (mut app, _, container) = anchored_app(Node {
        position_type: PositionType::Absolute,
        left: px(400),
        top: px(200),
        width: px(800),
        height: px(400),
        ..default()
    });
    let child = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(50),
                top: px(30),
                width: px(100),
                height: px(60),
                ..default()
            },
            ChildOf(container),
        ))
        .id();
    settle(&mut app);

    select_all(&mut app, &[container, child]);
    settle(&mut app);

    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);

    assert_eq!(
        node_of(&app, container).left,
        px(401),
        "the container takes the nudge",
    );
    assert_eq!(
        node_of(&app, child).left,
        px(50),
        "and the child rides along inside it, rather than being moved again",
    );
}

/// A nudge writes through the same projection a drag does: the node
/// keeps the offsets its author wrote, and never grows a `Transform`,
/// which belongs to the 3D writer.
#[test]
fn a_nudged_node_keeps_its_authored_offsets() {
    let (mut app, _, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        right: percent(10),
        bottom: px(300),
        width: px(400),
        height: px(200),
        ..default()
    });
    select(&mut app, node);
    settle(&mut app);

    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    let nudged = node_of(&app, node);
    assert_eq!(
        (nudged.right, nudged.left, nudged.bottom),
        (percent(9.96), Val::Auto, px(300)),
        "a pixel right of a 2400-wide parent is a twenty-fourth of a point off the far offset",
    );
    assert!(
        app.world().get::<Transform>(node).is_none(),
        "a canvas nudge never reaches for the 3D writer's component",
    );
}

/// In `Interact` the keys belong to the scene, so an arrow nudges
/// nothing and pushes no history entry.
#[test]
fn a_canvas_in_interact_mode_does_not_nudge() {
    let (mut app, panel, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        left: px(400),
        top: px(200),
        width: px(400),
        height: px(200),
        ..default()
    });
    select(&mut app, node);
    settle(&mut app);
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .mode = Viewport2dMode::Interact;
    let before = node_of(&app, node);
    let entries = history_len(&app);

    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    assert_eq!(node_of(&app, node), before, "the node stayed where it was");
    assert_eq!(
        history_len(&app),
        entries,
        "and nothing reached the history"
    );
}

/// Arrow keys reach a focused text field, not the canvas. The guard is
/// `keybind_focus::KeybindFocus`, applied through the nudge operator's
/// availability, so the canvas nudge sits behind it rather than beside
/// it.
#[test]
fn a_focused_text_field_keeps_the_arrow_keys() {
    let (mut app, _, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        left: px(400),
        top: px(200),
        width: px(400),
        height: px(200),
        ..default()
    });
    select(&mut app, node);
    settle(&mut app);
    let before = node_of(&app, node);

    let field = app
        .world_mut()
        .spawn(jackdaw_feathers::text_edit::EditorTextEdit)
        .id();
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .set(field, bevy::input_focus::FocusCause::Navigated);
    settle(&mut app);

    let _ = app.world_mut().operator("transform.nudge_x_pos").call();
    settle(&mut app);
    assert_eq!(
        node_of(&app, node),
        before,
        "the arrow went to the field the user is typing in",
    );
}

/// The other half of the routing: a selection with a `Transform` reaches
/// the 3D writer, on the world grid. The canvas nudge answers first, so
/// it has to decline a selection that is not on a canvas.
#[test]
fn a_transform_selection_still_nudges_in_world_units() {
    let mut app = stage_app();
    let entity = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::default(),
        ))
        .id();
    select(&mut app, entity);
    settle(&mut app);
    let grid = app
        .world()
        .resource::<jackdaw::snapping::SnapSettings>()
        .grid_size();

    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    assert_eq!(
        app.world()
            .get::<Transform>(entity)
            .expect("the entity keeps its transform")
            .translation
            .x,
        grid,
        "a 3D selection moves by the world grid, not by an authored pixel",
    );
}

/// Dispatch one nudge operator, the way its arrow-key binding does.
fn nudge(app: &mut App, operator: &'static str) {
    app.world_mut()
        .operator(operator)
        .call()
        .expect("the nudge operator is registered")
        .assert_finished();
    app.update();
}

fn set_grid(app: &mut App, panel: Entity, grid: f32) {
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .grid = grid;
}

/// Hold Shift the way the input pass reports it, so the next nudge reads
/// the coarse step.
fn hold_shift(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
}

// ---------------------------------------------------------------------------
// A move carries the whole selection
// ---------------------------------------------------------------------------

/// Every selected node moves, and the whole gesture is one entry, so one
/// undo puts all of them back.
#[test]
fn a_drag_moves_every_selected_node_and_undoes_as_one() {
    let (mut app, panel, nodes) = selection_app();
    let [first, second, third] = nodes;
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    select_all(&mut app, &nodes);
    settle(&mut app);
    let before: Vec<Node> = nodes.iter().map(|node| node_of(&app, *node)).collect();
    let entries = history_len(&app);

    let (overlay, _) = overlay_node(&mut app);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(1300.0, 500.0),
        Vec2::new(1360.0, 530.0),
    );
    settle(&mut app);

    assert_eq!(
        [
            (node_of(&app, first).left, node_of(&app, first).top),
            (node_of(&app, second).left, node_of(&app, second).top),
            (node_of(&app, third).left, node_of(&app, third).top),
        ],
        [(px(260), px(130)), (px(760), px(430)), (px(1260), px(730)),],
        "the drag moved all three by the same delta, not only the one under the cursor",
    );
    assert_eq!(
        history_len(&app),
        entries + 1,
        "one gesture is one entry however many nodes it moved",
    );

    undo(&mut app);
    settle(&mut app);
    assert_eq!(
        nodes
            .iter()
            .map(|node| node_of(&app, *node))
            .collect::<Vec<_>>(),
        before,
        "one undo puts the whole selection back",
    );
}

/// The primary is what lands on a neighbouring edge, and the rest of the
/// selection moves by the delta it landed with. Snapping each node to
/// its own neighbours would pull the arrangement apart: two nodes eight
/// pixels apart would come to rest on two different edges and stop being
/// eight pixels apart.
#[test]
fn a_snapped_multi_move_keeps_the_selection_arranged() {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    without_the_pixel_grid(&mut app, panel);
    let root = ui_root(&mut app);
    // What the mover lands on.
    spawn_child(&mut app, root, 1000.0, 0.0, 100.0, 100.0);
    let companion = spawn_child(&mut app, root, 900.0, 700.0, 200.0, 100.0);
    let mover = spawn_child(&mut app, root, 400.0, 400.0, 400.0, 200.0);
    settle(&mut app);

    select_all(&mut app, &[companion, mover]);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    // Ten authored pixels short of the sibling's left edge at 1000.
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(600.0, 500.0),
        Vec2::new(1190.0, 500.0),
    );
    settle(&mut app);

    assert_eq!(
        node_of(&app, mover).left,
        px(1000),
        "the node under the cursor landed on the sibling's edge",
    );
    assert_eq!(
        node_of(&app, companion).left,
        px(1500),
        "and the rest of the selection moved by the same snapped delta",
    );
    assert_eq!(
        node_of(&app, companion).top,
        px(700),
        "an axis nothing snapped on does not move",
    );
}

// ---------------------------------------------------------------------------
// What a drag is offered, and what it writes
// ---------------------------------------------------------------------------

/// A sibling's centre line is a kind of its own, so a node can be
/// centred on its neighbour without the neighbour's edges getting in the
/// way -- and turning the kind off takes that line away again.
#[test]
fn a_move_lands_on_a_sibling_centre_when_that_kind_is_on() {
    let landed = |centres: bool| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        without_the_pixel_grid(&mut app, panel);
        with_kinds(&mut app, |kinds| kinds.sibling_centers = centres);
        let root = ui_root(&mut app);
        // Centre at authored x 400, clear of every parent and quarter
        // line of the 2400-wide canvas.
        spawn_child(&mut app, root, 200.0, 100.0, 400.0, 200.0);
        let mover = spawn_child(&mut app, root, 1000.0, 700.0, 100.0, 100.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(1050.0, 750.0),
            Vec2::new(444.0, 750.0),
        );
        settle(&mut app);
        node_of(&app, mover).left
    };

    assert_eq!(
        landed(true),
        px(400),
        "the sibling's centre pulls the move onto it",
    );
    assert_eq!(
        landed(false),
        px(394),
        "with the kind off the same drag stops where the cursor did",
    );
}

/// A kind is what puts its lines in front of a drag at all, rather than
/// a filter applied to a landing already chosen. An edge a switched-off
/// kind governs cannot claim a drag, and cannot win a tie against a kind
/// that is on either.
#[test]
fn a_kind_switched_off_frees_the_edge_it_governs() {
    let landed = |sides: bool| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        without_the_pixel_grid(&mut app, panel);
        with_kinds(&mut app, |kinds| kinds.sibling_sides = sides);
        let root = ui_root(&mut app);
        // Sides at 900 and 1100: neither is a quarter line of the
        // 2400-wide canvas, so only the sibling kind offers them.
        spawn_child(&mut app, root, 900.0, 100.0, 200.0, 200.0);
        let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(230.0, 750.0),
            Vec2::new(924.0, 750.0),
        );
        settle(&mut app);
        node_of(&app, mover).left
    };

    assert_eq!(landed(true), px(900), "the sibling's near edge claims it");
    assert_eq!(
        landed(false),
        px(894),
        "and nothing does once that kind is off",
    );
}

/// Ctrl is still one switch, and the kinds did not become a second one.
/// Ctrl inverts the master magnet for the length of a gesture, and what
/// that turns on is every kind of line and the pixel grid together.
#[test]
fn ctrl_still_inverts_the_kinds_together_with_the_grid() {
    // The magnet is off throughout: Ctrl is the whole switch here.
    let landed = |ctrl: bool, to: f32| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        magnet(&mut app, false);
        let root = ui_root(&mut app);
        spawn_child(&mut app, root, 900.0, 100.0, 200.0, 200.0);
        let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        if ctrl {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ControlLeft);
        }
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(230.0, 750.0),
            Vec2::new(to + 30.0, 750.0),
        );
        settle(&mut app);
        node_of(&app, mover).left
    };

    assert_eq!(
        landed(true, 894.0),
        px(900),
        "ctrl turns the sibling's edge on",
    );
    assert_eq!(
        landed(true, 1470.0),
        px(1472),
        "... and the same ctrl lands the same gesture on the grid out in the open",
    );
    assert_eq!(landed(false, 894.0), px(894), "without it, neither");
    assert_eq!(landed(false, 1470.0), px(1470), "on either kind of line");
}

/// A node whose author wrote its offset as a percentage and landed on a
/// quarter line writes that quarter outright.
///
/// The parent here is 1003 authored pixels wide, so a quarter of it is
/// 250.75: a figure that goes through the pixel path and comes back as
/// something that is not 25%, and would leave the node a hair off the
/// line at every other canvas size.
#[test]
fn a_percent_anchored_node_landing_on_a_quarter_line_writes_the_exact_percent() {
    // Near edge: the left offset takes the quarter it landed on.
    let (mut app, panel, child) = quarter_line_app(Node {
        position_type: PositionType::Absolute,
        left: percent(10),
        top: px(0),
        width: px(100),
        height: px(50),
        ..default()
    });
    let (overlay, _) = {
        select(&mut app, child);
        settle(&mut app);
        overlay_node(&mut app)
    };
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(150.0, 25.0),
        Vec2::new(300.0, 25.0),
    );
    settle(&mut app);
    assert_eq!(
        node_of(&app, child).left,
        Val::Percent(25.0),
        "the landing is written as the quarter it is, not as a figure from pixels",
    );

    // Far edge: `right` is measured back from the parent's far edge, so
    // a landing a quarter of the way in is three quarters of the way
    // back.
    let (mut app, panel, child) = quarter_line_app(Node {
        position_type: PositionType::Absolute,
        right: percent(60),
        top: px(150),
        width: px(100),
        height: px(50),
        ..default()
    });
    let (overlay, _) = {
        select(&mut app, child);
        settle(&mut app);
        overlay_node(&mut app)
    };
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(351.2, 175.0),
        Vec2::new(201.2, 175.0),
    );
    settle(&mut app);
    assert_eq!(
        node_of(&app, child).right,
        Val::Percent(75.0),
        "a far offset landing on the quarter line writes the rest of the box",
    );

    // A centre landing names no edge, so nothing is written outright.
    // A node centred on a quarter line sits half its own width back
    // from it, which is not a figure that line can supply.
    let (mut app, panel, child) = quarter_line_app(Node {
        position_type: PositionType::Absolute,
        left: percent(10),
        top: px(0),
        width: px(100),
        height: px(50),
        ..default()
    });
    let (overlay, _) = {
        select(&mut app, child);
        settle(&mut app);
        overlay_node(&mut app)
    };
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(150.0, 25.0),
        Vec2::new(250.75, 25.0),
    );
    settle(&mut app);
    let left = node_of(&app, child).left;
    assert!(
        matches!(left, Val::Percent(_)),
        "the offset stays the percentage its author wrote, got {left:?}",
    );
    assert_ne!(
        left,
        Val::Percent(25.0),
        "a centre landing must not put the line's own figure into a near offset",
    );
}

/// A canvas at 1003 authored pixels holding one child authored as
/// `node`. Returns the panel and the child.
fn quarter_line_app(node: Node) -> (App, Entity, Entity) {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    let root = ui_root(&mut app);
    let container = spawn_child(&mut app, root, 0.0, 0.0, 1003.0, 400.0);
    let child = app.world_mut().spawn((node, ChildOf(container))).id();
    settle(&mut app);
    (app, panel, child)
}

/// Pixel Snap is what decides how finely a drag states the pixels it
/// writes, and it is the only thing that decides: with it off, a canvas
/// zoomed past one authored pixel per pointer pixel keeps the fraction
/// the drag actually produced.
#[test]
fn pixel_snap_off_keeps_the_fraction_a_zoomed_drag_produced() {
    // At zoom 4 one pointer pixel is a quarter of an authored one.
    let landed = |pixel: bool| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 4.0);
        let root = ui_root(&mut app);
        let mover = spawn_child(&mut app, root, 400.0, 200.0, 100.0, 100.0);
        settle(&mut app);
        // The master off leaves the pixel kind as the only thing
        // deciding the figure: that it is a separate switch is the
        // point of this test.
        with_kinds(&mut app, |kinds| {
            kinds.enabled = false;
            kinds.pixel = pixel;
        });

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        let start = begin_drag(&mut app, panel, overlay, Vec2::new(450.0, 250.0));
        let distance = Vec2::new(1.0, 0.0);
        continue_drag(&mut app, overlay, start, distance);
        end_drag(&mut app, overlay, start, distance);
        settle(&mut app);
        node_of(&app, mover).left
    };

    assert_eq!(
        landed(false),
        px(400.25),
        "one pointer pixel at this zoom is a quarter of an authored one",
    );
    assert_eq!(
        landed(true),
        px(400),
        "and Pixel Snap rounds that back onto the canvas's own lattice",
    );
}

/// Other Nodes reaches outside the dragged node's family, and ships off
/// because it pulls a node towards something the user cannot see beside
/// it. What it never reaches is the selection's own descendants: layout
/// carries those with the drag, so none of them holds still to land on.
#[test]
fn other_nodes_reach_across_the_tree_only_when_asked() {
    let landed = |other_nodes: bool, to: f32| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        without_the_pixel_grid(&mut app, panel);
        with_kinds(&mut app, |kinds| kinds.other_nodes = other_nodes);
        let root = ui_root(&mut app);
        let branch = spawn_child(&mut app, root, 0.0, 0.0, 1200.0, 1200.0);
        // Another branch entirely: its near edge is at authored 1400.
        spawn_child(&mut app, root, 1400.0, 0.0, 400.0, 400.0);
        let mover = spawn_child(&mut app, branch, 200.0, 700.0, 60.0, 100.0);
        // The mover's own child, laid out at authored 1000 to 1020.
        spawn_child(&mut app, mover, 800.0, 0.0, 20.0, 20.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        drag_authored(
            &mut app,
            panel,
            overlay,
            Vec2::new(230.0, 750.0),
            Vec2::new(to + 30.0, 750.0),
        );
        settle(&mut app);
        node_of(&app, mover).left
    };

    assert_eq!(
        landed(true, 1394.0),
        px(1400),
        "a node in another branch claims the drag once the kind is on",
    );
    assert_eq!(
        landed(false, 1394.0),
        px(1394),
        "and does not while it is off",
    );
    assert_eq!(
        landed(true, 994.0),
        px(994),
        "the node the drag is carrying never offers its own descendants",
    );
}

/// The keyboard nudge is kind-blind. It is one authored pixel a press
/// however close the node is to a line the canvas would offer a drag, so
/// the arrows stay a way of saying an exact number rather than a second
/// way of landing on something.
#[test]
fn a_nudge_ignores_the_snap_kinds() {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    let root = ui_root(&mut app);
    // Four authored pixels to the right of the mover's near edge: well
    // inside the radius a drag would land from.
    spawn_child(&mut app, root, 404.0, 100.0, 100.0, 100.0);
    let mover = spawn_child(&mut app, root, 400.0, 700.0, 100.0, 100.0);
    settle(&mut app);
    set_grid(&mut app, panel, 8.0);

    select(&mut app, mover);
    settle(&mut app);
    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    assert_eq!(
        node_of(&app, mover).left,
        px(401),
        "one press is one authored pixel, not a landing on the near edge",
    );

    // ... and with every kind switched off it is still that one pixel.
    with_kinds(&mut app, |kinds| {
        for kind in CanvasSnapKind::ALL {
            kinds.set(kind, false);
        }
    });
    nudge(&mut app, "transform.nudge_x_pos");
    settle(&mut app);
    assert_eq!(
        node_of(&app, mover).left,
        px(402),
        "and one whole pixel again with nothing on offer",
    );
}

/// The gesture keeps the line it came to rest against, so the canvas can
/// draw it, and lets go of it on release.
#[test]
fn the_gesture_records_the_winning_line() {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    without_the_pixel_grid(&mut app, panel);
    let root = ui_root(&mut app);
    spawn_child(&mut app, root, 900.0, 100.0, 200.0, 200.0);
    let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
    settle(&mut app);

    select(&mut app, mover);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let start = begin_drag(&mut app, panel, overlay, Vec2::new(230.0, 750.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(924.0, 750.0)) - start;
    continue_drag(&mut app, overlay, start, distance);
    settle(&mut app);

    let outcome = app.world().resource::<UiManipulation>().last_snap();
    let won = outcome.x.expect("the x axis landed on the sibling's edge");
    assert_eq!(won.kind, CandidateKind::SiblingSide);
    assert_eq!(won.line, SnapLine::Min);
    assert_eq!(won.at, 900.0, "the line is stated where the candidate is");
    assert_eq!(won.percent, None, "a sibling edge is no fraction of a box");
    assert!(
        (outcome.nudge.x - 6.0).abs() < 1e-3,
        "and the nudge is what puts the edge on it, got {:?}",
        outcome.nudge,
    );
    assert_eq!(outcome.y, None, "nothing claimed the other axis");

    end_drag(&mut app, overlay, start, distance);
    settle(&mut app);
    assert_eq!(
        app.world().resource::<UiManipulation>().last_snap(),
        SnapOutcome::default(),
        "the release lets go of the line",
    );
}

/// Edit the canvas's snap kinds, the way the header's Snap menu does.
fn with_kinds(app: &mut App, edit: impl FnOnce(&mut CanvasSnap)) {
    edit(&mut app.world_mut().resource_mut::<CanvasSnap>());
}

// ---------------------------------------------------------------------------
// The line a drag came to rest against
// ---------------------------------------------------------------------------

/// A drag that landed on something says so: a line across the stage
/// where the landing is, for as long as the gesture holds it.
///
/// The line is drawn on the canvas, and a candidate is stated from the
/// dragged node's own parent, so the parent's corner is part of the
/// position. Leaving it out puts the line in the right place only while
/// the parent happens to be the canvas root.
#[test]
fn a_snapped_drag_draws_the_line_it_landed_on_and_lets_go_of_it() {
    // The dragged node's parent is the root, so the landing is already
    // a canvas position: authored 900, half of that in stage pixels.
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    without_the_pixel_grid(&mut app, panel);
    let root = ui_root(&mut app);
    spawn_child(&mut app, root, 900.0, 100.0, 200.0, 200.0);
    let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
    settle(&mut app);

    select(&mut app, mover);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let start = begin_drag(&mut app, panel, overlay, Vec2::new(230.0, 750.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(924.0, 750.0)) - start;
    continue_drag(&mut app, overlay, start, distance);
    settle(&mut app);

    assert_eq!(
        snap_highlights(&mut app),
        vec![(CanvasAxis::Vertical, px(450))],
        "one line, down the canvas, where the near edge landed",
    );
    let (line, _) = snap_highlight_entities(&mut app)[0];
    assert!(
        app.world().get::<jackdaw::EditorEntity>(line).is_some(),
        "the line is editor chrome, never part of the authored tree",
    );
    assert_eq!(
        app.world().get::<ChildOf>(line).map(ChildOf::parent),
        Some(stage_entity(&mut app, panel)),
        "and it is parented into the stage beside the outline",
    );

    end_drag(&mut app, overlay, start, distance);
    settle(&mut app);
    assert!(
        snap_highlights(&mut app).is_empty(),
        "the release lets go of the line",
    );

    // The same canvas position reached from a parent that is not the
    // root: the container sits at authored 300, and the landing is 600
    // measured from inside it.
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    without_the_pixel_grid(&mut app, panel);
    let root = ui_root(&mut app);
    let container = spawn_child(&mut app, root, 300.0, 200.0, 1600.0, 900.0);
    spawn_child(&mut app, container, 600.0, 0.0, 200.0, 200.0);
    let mover = spawn_child(&mut app, container, 0.0, 500.0, 60.0, 100.0);
    settle(&mut app);

    select(&mut app, mover);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let start = begin_drag(&mut app, panel, overlay, Vec2::new(330.0, 750.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(924.0, 750.0)) - start;
    continue_drag(&mut app, overlay, start, distance);
    settle(&mut app);

    assert_eq!(
        node_of(&app, mover).left,
        px(600),
        "the drag landed on the sibling's edge inside the container",
    );
    assert_eq!(
        snap_highlights(&mut app),
        vec![(CanvasAxis::Vertical, px(450))],
        "and the line is drawn at the canvas position that landing is",
    );
}

/// A drag that came to rest on nothing draws nothing. The grid is not a
/// line the canvas can point at: it is everywhere, so a line on it says
/// nothing about why the node stopped there.
#[test]
fn a_drag_that_snapped_nothing_draws_nothing() {
    let mut app = stage_app();
    let panel = framed_panel(&mut app, 0.5);
    let root = ui_root(&mut app);
    let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
    settle(&mut app);
    set_grid(&mut app, panel, 8.0);

    select(&mut app, mover);
    settle(&mut app);
    let (overlay, _) = overlay_node(&mut app);
    let start = begin_drag(&mut app, panel, overlay, Vec2::new(230.0, 750.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(1500.0, 750.0)) - start;
    continue_drag(&mut app, overlay, start, distance);
    settle(&mut app);

    assert_eq!(
        node_of(&app, mover).left,
        px(1472),
        "the drag did land on the grid, out in the open",
    );
    assert!(
        snap_highlights(&mut app).is_empty(),
        "but the grid is not a line the canvas points at",
    );
}

/// The snap highlights on screen, with the axis each one runs along.
fn snap_highlight_entities(app: &mut App) -> Vec<(Entity, CanvasAxis)> {
    app.world_mut()
        .query::<(Entity, &SnapHighlight)>()
        .iter(app.world())
        .map(|(entity, highlight)| (entity, highlight.axis))
        .collect()
}

/// Every snap highlight on screen: which way it runs, and where it sits
/// in the stage's own logical pixels.
fn snap_highlights(app: &mut App) -> Vec<(CanvasAxis, Val)> {
    snap_highlight_entities(app)
        .into_iter()
        .map(|(entity, axis)| {
            let node = app.world().get::<Node>(entity).expect("the line is a node");
            let at = match axis {
                CanvasAxis::Vertical => node.left,
                CanvasAxis::Horizontal => node.top,
            };
            (axis, at)
        })
        .collect()
}

/// The landing line is its own colour, whatever it landed on.
///
/// The selection outline is drawn over the node in `ACCENT_BLUE` and a
/// guide is drawn under it in `GUIDE_LINE`; a landing painted in either
/// would be a line that reads as the thing beside it rather than as the
/// answer to "why did the drag stop here".
#[test]
fn the_landing_line_is_told_apart_from_the_outline_and_the_guides() {
    let painted = |guide: bool| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        without_the_pixel_grid(&mut app, panel);
        let root = ui_root(&mut app);
        if guide {
            app.world_mut().entity_mut(root).insert(CanvasGuides {
                horizontal: Vec::new(),
                vertical: vec![900.0],
            });
        } else {
            spawn_child(&mut app, root, 900.0, 100.0, 200.0, 200.0);
        }
        let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        let start = begin_drag(&mut app, panel, overlay, Vec2::new(230.0, 750.0));
        let distance = screen_position_of(&mut app, panel, Vec2::new(924.0, 750.0)) - start;
        continue_drag(&mut app, overlay, start, distance);
        settle(&mut app);
        let (entity, _) = snap_highlight_entities(&mut app)
            .into_iter()
            .next()
            .expect("the drag landed on something and drew the line");
        app.world()
            .get::<BackgroundColor>(entity)
            .expect("the line is painted")
            .0
    };

    for (landing, colour) in [("a sibling", painted(false)), ("a guide", painted(true))] {
        assert_ne!(
            colour,
            jackdaw_feathers::tokens::ACCENT_BLUE,
            "a landing on {landing} is not the selection outline's colour",
        );
        assert_ne!(
            colour,
            jackdaw_feathers::tokens::GUIDE_LINE,
            "nor a guide's, which it would be drawn straight on top of",
        );
    }
}

/// A guide is a line the author drew, and a drag lands on it like any
/// other: its own kind, so it can be switched off without taking the
/// parent and the siblings with it.
#[test]
fn a_drag_lands_on_a_guide_when_that_kind_is_on() {
    let landed = |guides: bool| {
        let mut app = stage_app();
        let panel = framed_panel(&mut app, 0.5);
        without_the_pixel_grid(&mut app, panel);
        with_kinds(&mut app, |kinds| kinds.guides = guides);
        let root = ui_root(&mut app);
        // Nothing else in the scene sits at 900: only the guide offers it.
        app.world_mut().entity_mut(root).insert(CanvasGuides {
            horizontal: Vec::new(),
            vertical: vec![900.0],
        });
        let mover = spawn_child(&mut app, root, 200.0, 700.0, 60.0, 100.0);
        settle(&mut app);

        select(&mut app, mover);
        settle(&mut app);
        let (overlay, _) = overlay_node(&mut app);
        let start = begin_drag(&mut app, panel, overlay, Vec2::new(230.0, 750.0));
        let distance = screen_position_of(&mut app, panel, Vec2::new(924.0, 750.0)) - start;
        continue_drag(&mut app, overlay, start, distance);
        settle(&mut app);
        let outcome = app.world().resource::<UiManipulation>().last_snap();
        end_drag(&mut app, overlay, start, distance);
        settle(&mut app);
        (node_of(&app, mover).left, outcome.x.map(|won| won.kind))
    };

    assert_eq!(
        landed(true),
        (px(900), Some(CandidateKind::Guide)),
        "the guide claims the drag, and says it was a guide that did",
    );
    assert_eq!(
        landed(false),
        (px(894), None),
        "and offers nothing once the kind is off",
    );
}

/// Three absolutely placed children, 500 authored pixels apart on both
/// axes. Returns the panel and the three, in tree order.
fn selection_app() -> (App, Entity, [Entity; 3]) {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let root = ui_root(&mut app);
    let nodes = [
        spawn_child(&mut app, root, 200.0, 100.0, 300.0, 150.0),
        spawn_child(&mut app, root, 700.0, 400.0, 300.0, 150.0),
        spawn_child(&mut app, root, 1200.0, 700.0, 300.0, 150.0),
    ];
    settle(&mut app);
    (app, panel, nodes)
}

/// A root filling the canvas, with nothing in it yet.
fn ui_root(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id()
}

fn spawn_child(
    app: &mut App,
    root: Entity,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) -> Entity {
    app.world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(left),
                top: px(top),
                width: px(width),
                height: px(height),
                ..default()
            },
            ChildOf(root),
        ))
        .id()
}

/// Select all of `entities`, the last one primary: the outline is drawn
/// around the primary, and the primary is what the gesture snaps.
fn select_all(app: &mut App, entities: &[Entity]) {
    app.world_mut().resource_mut::<Selection>().entities = entities.to_vec();
}

// ---------------------------------------------------------------------------
// Manipulation preserves the scheme the node was authored in
// ---------------------------------------------------------------------------

/// The projection on its own, one axis at a time. A drag computes a rect
/// in authored pixels and then writes it back through the scheme it
/// found, and this is that second half with no pointer in the way.
#[test]
fn a_rect_is_written_back_through_the_scheme_it_was_read_from() {
    let basis = UnitBasis {
        parent: Vec2::new(1000.0, 500.0),
        viewport: Vec2::new(2400.0, 1200.0),
    };
    // The rect a move by (30, 20) leaves behind, for a node that started
    // at (100, 50) sized 200x100.
    let moved = Vec4::new(130.0, 70.0, 200.0, 100.0);

    // Left and top: the near offsets take the delta directly.
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: px(100),
        top: px(50),
        width: px(200),
        height: px(100),
        ..default()
    };
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        moved,
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        (node.left, node.top, node.right, node.bottom),
        (px(130), px(70), Val::Auto, Val::Auto),
        "a node placed from the near edges keeps being placed from them",
    );

    // Right and bottom: the offsets move by the negated delta, and the
    // near edges are never given a number they did not have.
    let mut node = Node {
        position_type: PositionType::Absolute,
        right: px(700),
        bottom: px(350),
        width: px(200),
        height: px(100),
        ..default()
    };
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        moved,
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        (node.left, node.top, node.right, node.bottom),
        (Val::Auto, Val::Auto, px(670), px(330)),
        "a node pinned to the far edges stays pinned to them",
    );

    // Percentages stay percentages: the delta goes through the parent's
    // size into percentage points rather than rewriting the unit.
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: percent(10),
        top: percent(10),
        width: percent(20),
        height: percent(20),
        ..default()
    };
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        moved,
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        (node.left, node.top),
        (percent(13), percent(14)),
        "30 of 1000 is three points, 20 of 500 is four",
    );

    // A stretch keeps both edges and gains no size: the size is what the
    // two offsets leave between them, and writing one would pin it.
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: px(100),
        right: px(700),
        top: px(50),
        bottom: px(350),
        ..default()
    };
    let stretched = Vec4::new(130.0, 70.0, 200.0, 100.0);
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        stretched,
        (1, 1),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        (node.left, node.right, node.width),
        (px(130), px(670), Val::Auto),
        "both edges move and the size stays derived, even on a resize",
    );
}

/// A unit with nothing measurable behind it is left alone rather than
/// rewritten in pixels, so that a parent measuring zero for one frame
/// cannot turn `50%` into `Val::Px`.
#[test]
fn a_unit_with_no_basis_is_left_as_it_was() {
    let basis = UnitBasis {
        parent: Vec2::ZERO,
        viewport: Vec2::new(2400.0, 1200.0),
    };
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: percent(10),
        right: percent(20),
        ..default()
    };
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        Vec4::new(130.0, 70.0, 200.0, 100.0),
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        (node.left, node.right),
        (percent(10), percent(20)),
        "no parent size is no conversion, and no conversion is no rewrite",
    );

    // Viewport units have the canvas to measure against, so they convert
    // whatever the parent is doing.
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: Val::Vw(5.0),
        ..default()
    };
    let anchors = NodeAnchors::of(&node);
    apply_authored_rect(
        &mut node,
        anchors,
        Vec4::new(240.0, 0.0, 200.0, 100.0),
        (0, 0),
        basis,
        PixelRounding::Whole,
        ExactPercent::default(),
    );
    assert_eq!(
        node.left,
        Val::Vw(10.0),
        "240 of a 2400-wide canvas is 10vw"
    );
}

/// The same three schemes through a real drag, against real layout.
#[test]
fn a_drag_edits_the_offsets_the_node_was_authored_with() {
    // Pinned to the bottom-right corner: writing `left`/`top` here would
    // move the node to the opposite corner of the parent.
    let (mut app, panel, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        right: px(400),
        bottom: px(300),
        width: px(400),
        height: px(200),
        ..default()
    });
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    let overlay = outline_over(&mut app, node);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(1800.0, 800.0),
        Vec2::new(1860.0, 830.0),
    );
    settle(&mut app);
    let dragged = node_of(&app, node);
    assert_eq!(
        (
            dragged.left,
            dragged.top,
            dragged.right,
            dragged.bottom,
            dragged.width,
            dragged.height,
        ),
        (Val::Auto, Val::Auto, px(340), px(270), px(400), px(200)),
        "the drag moved the offsets the author wrote, by the negated delta",
    );

    // Percentages: the same drag in percentage points.
    let (mut app, panel, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        left: percent(10),
        top: percent(10),
        width: percent(20),
        height: percent(20),
        ..default()
    });
    let overlay = outline_over(&mut app, node);
    // The node is at (240, 120) sized 480x240 in a 2400x1200 canvas.
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(400.0, 200.0),
        Vec2::new(448.0, 224.0),
    );
    settle(&mut app);
    let dragged = node_of(&app, node);
    assert_eq!(
        (dragged.left, dragged.top, dragged.width, dragged.height),
        (percent(12), percent(12), percent(20), percent(20)),
        "48 of 2400 and 24 of 1200 are two points each, and the size is untouched",
    );

    // Stretched across one axis and pinned from the top on the other:
    // each axis is projected through its own scheme.
    let (mut app, panel, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        left: px(100),
        right: px(100),
        top: px(50),
        height: px(200),
        ..default()
    });
    let overlay = outline_over(&mut app, node);
    drag_authored(
        &mut app,
        panel,
        overlay,
        Vec2::new(1200.0, 100.0),
        Vec2::new(1260.0, 130.0),
    );
    settle(&mut app);
    let dragged = node_of(&app, node);
    assert_eq!(
        (
            dragged.left,
            dragged.right,
            dragged.width,
            dragged.top,
            dragged.bottom,
            dragged.height,
        ),
        (px(160), px(40), Val::Auto, px(80), Val::Auto, px(200)),
        "the stretched axis moves both edges; the pinned one moves only its own",
    );
}

/// Resizing a node pinned to its far edge moves the near edge through
/// its size: the far offset is where the author put it, and a resize
/// from the other side must not walk it.
#[test]
fn a_resize_of_a_far_pinned_node_leaves_the_far_offset_alone() {
    let (mut app, panel, node) = anchored_app(Node {
        position_type: PositionType::Absolute,
        right: px(400),
        bottom: px(300),
        width: px(400),
        height: px(200),
        ..default()
    });
    // A gesture read for what it writes, not for what it lands on:
    // the magnet off is the only way to see the cursor's own figures.
    magnet(&mut app, false);
    let overlay = outline_over(&mut app, node);
    // The node's left edge is at 2400 - 400 - 400 = 1600.
    let handle = handle_entity(&mut app, overlay, (-1, 0));
    drag_authored(
        &mut app,
        panel,
        handle,
        Vec2::new(1600.0, 800.0),
        Vec2::new(1700.0, 800.0),
    );
    settle(&mut app);
    let resized = node_of(&app, node);
    assert_eq!(
        (resized.right, resized.width, resized.left),
        (px(400), px(300), Val::Auto),
        "the far edge held still and the size took the whole drag",
    );
}

#[test]
fn rulers_sit_outside_the_area_the_stage_is_measured_against() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    settle(&mut app);

    assert_eq!(
        area_size(&app, panel),
        Vec2::new(1200.0, 600.0),
        "the gutter comes off the panel the way the header does",
    );
    let area = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .area;
    for (ruler, _) in rulers_of(&mut app, panel) {
        assert!(
            !descends_from(&app, ruler, area),
            "a ruler inside the area would be clipped with the canvas and fake a stage hover",
        );
    }

    // The area is still what a fit is computed against, so the framing
    // the panel arrives at is the one that area asks for.
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .fit_pending = true;
    settle(&mut app);
    let view = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view;
    assert_eq!(
        view.zoom,
        jackdaw::viewport_2d::fit_view(view, REFERENCE, Vec2::new(1200.0, 600.0)).zoom,
        "the fit is the one the 1200x600 area asks for",
    );
}

#[test]
fn show_rulers_off_gives_the_area_the_gutter_back() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    settle(&mut app);

    app.world_mut().resource_mut::<CanvasSnap>().show_rulers = false;
    settle(&mut app);
    assert_eq!(
        area_size(&app, panel),
        Vec2::new(1200.0 + RULER_SIZE, 600.0 + RULER_SIZE),
        "a hidden gutter takes no room from the canvas",
    );

    app.world_mut().resource_mut::<CanvasSnap>().show_rulers = true;
    settle(&mut app);
    assert_eq!(
        area_size(&app, panel),
        Vec2::new(1200.0, 600.0),
        "and comes back the size it was",
    );
}

/// The ruler is a reading of the canvas as the panel is showing it: the
/// figures follow the pan, not just the zoom.
#[test]
fn ruler_labels_count_every_hundred_authored_pixels_at_the_zoom() {
    // A canvas origin a hundred pixels off the left of the ruler, at
    // half zoom: the ruler's 1200 pixels read 200 to 2600 authored.
    let marks = ruler_marks(-100.0, 0.5, 1200.0);
    let labels: Vec<f32> = marks
        .iter()
        .filter(|mark| mark.labelled)
        .map(|mark| mark.authored)
        .collect();
    assert_eq!(
        labels.len(),
        25,
        "one label per hundred authored pixels on the ruler: {labels:?}",
    );
    assert_eq!(
        (labels.first().copied(), labels.last().copied()),
        (Some(200.0), Some(2600.0)),
    );
    assert!(
        marks.iter().any(|mark| !mark.labelled),
        "with the tens ten pixels apart there are ticks between the labels",
    );

    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    // Pan a hundred stage pixels left, which is the origin above.
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent");
    host.view.pan.x = 200.0;
    settle(&mut app);

    let drawn = ruler_labels(&mut app, panel, CanvasAxis::Vertical);
    assert_eq!(
        drawn.len(),
        25,
        "the ruler draws the labels the reading asks for: {drawn:?}",
    );
    assert_eq!(
        drawn.first().map(|(text, at)| (text.as_str(), *at)),
        Some(("200", 2.0)),
        "and reads the authored pixel under it, pan included",
    );
}

/// A ruler reads to its far end however wide the panel is.
///
/// A fixed ceiling on the number of marks stops them partway across, and
/// the panel past that point reads as canvas with no ruler over it.
#[test]
fn a_wide_ruler_is_marked_all_the_way_to_its_far_end() {
    // The densest reading the ruler offers: ten marks per label with the
    // marks exactly RULER_TICK_GAP apart.
    let marks = ruler_marks(0.0, 0.4, 4000.0);
    let last = marks.last().expect("a 4000 pixel ruler carries marks");
    assert!(
        last.at > 3990.0,
        "the marks reach the ruler's far end, got {} of them ending at {}",
        marks.len(),
        last.at,
    );
    assert!(
        marks.iter().any(|mark| !mark.labelled),
        "and they are still the dense reading, ticks between the labels",
    );
}

/// Panning moves the marks, it does not make new ones.
///
/// Comparing what a ruler was drawn for as a float means every frame of
/// a pan is a different reading and every mark on both rulers is
/// despawned and respawned, all the way through the drag.
#[test]
fn panning_moves_a_rulers_marks_rather_than_making_them_again() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    authored_scene(&mut app);
    // Both framings put the canvas origin between two marks, so the pan
    // between them moves every mark and brings no new figure in: what is
    // being read here is whether an unchanged reading is redrawn.
    pan_to(&mut app, panel, -5.0);
    settle(&mut app);

    let before = ruler_mark_entities(&mut app, panel, CanvasAxis::Vertical);
    assert!(!before.is_empty(), "the ruler starts out marked");

    pan_to(&mut app, panel, -3.0);
    settle(&mut app);

    assert_eq!(
        ruler_mark_entities(&mut app, panel, CanvasAxis::Vertical),
        before,
        "the same nodes carry the reading, moved to where the pan put them",
    );
}

/// Every guide is marked on the ruler it came off, on every panel.
///
/// A guide is put away by dragging it back onto its ruler, so the ruler
/// has to show where each one is.
#[test]
fn each_guide_is_marked_on_its_ruler() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    app.world_mut().entity_mut(root).insert(CanvasGuides {
        horizontal: vec![180.0, 400.0],
        vertical: vec![320.0],
    });
    settle(&mut app);

    assert_eq!(
        guide_marks_on(&mut app, panel, CanvasAxis::Vertical).len(),
        1,
        "the ruler along the top marks the guide down the canvas",
    );
    assert_eq!(
        guide_marks_on(&mut app, panel, CanvasAxis::Horizontal).len(),
        2,
        "and the one down the left marks both guides across it",
    );

    app.world_mut().resource_mut::<CanvasSnap>().show_guides = false;
    settle(&mut app);
    assert!(
        guide_marks_on(&mut app, panel, CanvasAxis::Vertical).is_empty(),
        "guides that are not drawn are not marked either",
    );
}

/// Slide the panel's view along the canvas's x axis.
fn pan_to(app: &mut App, panel: Entity, pan: f32) {
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .pan
        .x = pan;
}

/// The nodes carrying one ruler's marks, in the order it spawned them.
fn ruler_mark_entities(app: &mut App, panel: Entity, axis: CanvasAxis) -> Vec<Entity> {
    let ruler = ruler_entity(app, panel, axis);
    app.world()
        .get::<Children>(ruler)
        .map(|children| children.iter().collect())
        .unwrap_or_default()
}

/// The guide marks drawn on one of a panel's rulers.
fn guide_marks_on(app: &mut App, panel: Entity, axis: CanvasAxis) -> Vec<usize> {
    let mut query = app.world_mut().query::<&RulerGuideMark>();
    query
        .iter(app.world())
        .filter(|mark| mark.host == panel && mark.axis == axis)
        .map(|mark| mark.index)
        .collect()
}

#[test]
fn every_panel_showing_the_scene_draws_its_guides() {
    let mut app = stage_app();
    let first = panel_entity(&mut app);
    let second = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    app.world_mut().entity_mut(root).insert(CanvasGuides {
        horizontal: vec![180.0],
        vertical: vec![320.0],
    });
    settle(&mut app);

    assert_eq!(
        guide_lines_of(&mut app, first),
        vec![
            (CanvasAxis::Vertical, px(320.0 * 0.5 - 3.5)),
            (CanvasAxis::Horizontal, px(180.0 * 0.5 - 3.5)),
        ],
        "the scene's guides are drawn over the canvas at the panel's scale",
    );
    assert_eq!(
        guide_lines_of(&mut app, second).len(),
        2,
        "a second panel showing the same scene draws the same guides",
    );

    app.world_mut().resource_mut::<CanvasSnap>().show_guides = false;
    settle(&mut app);
    assert!(
        guide_lines_of(&mut app, first).is_empty(),
        "hidden guides are drawn nowhere",
    );

    app.world_mut().resource_mut::<CanvasSnap>().show_guides = true;
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(first)
        .expect("host on panel parent")
        .mode = Viewport2dMode::Interact;
    settle(&mut app);
    assert!(
        guide_lines_of(&mut app, first).is_empty(),
        "a panel running the scene has no editor lines over it",
    );
    assert_eq!(
        guide_lines_of(&mut app, second).len(),
        2,
        "while the panel still being authored keeps them",
    );
}

/// The panel's stage area as it was laid out, in logical pixels.
fn area_size(app: &App, panel: Entity) -> Vec2 {
    let area = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .area;
    let computed = app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out");
    computed.size() * computed.inverse_scale_factor()
}

fn rulers_of(app: &mut App, panel: Entity) -> Vec<(Entity, CanvasAxis)> {
    let mut query = app.world_mut().query::<(Entity, &CanvasRuler)>();
    query
        .iter(app.world())
        .filter(|(_, ruler)| ruler.host == panel)
        .map(|(entity, ruler)| (entity, ruler.axis))
        .collect()
}

fn descends_from(app: &App, entity: Entity, ancestor: Entity) -> bool {
    let mut cursor = entity;
    while let Some(parent) = app.world().get::<ChildOf>(cursor).map(ChildOf::parent) {
        if parent == ancestor {
            return true;
        }
        cursor = parent;
    }
    false
}

/// What one of a panel's rulers has written on it: each label and how
/// far along the ruler it sits, in order.
fn ruler_labels(app: &mut App, panel: Entity, axis: CanvasAxis) -> Vec<(String, f32)> {
    let ruler = rulers_of(app, panel)
        .into_iter()
        .find(|(_, ruler_axis)| *ruler_axis == axis)
        .map(|(entity, _)| entity)
        .expect("the panel has a ruler on each axis");
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(ruler)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let mut labels: Vec<(String, f32)> = children
        .into_iter()
        .filter_map(|child| {
            // The figure sits in a box of its own, which is what the
            // left ruler turns on its side.
            let text = app
                .world()
                .get::<Children>(child)
                .and_then(|children| children.iter().next())
                .and_then(|text| app.world().get::<Text>(text))?
                .0
                .clone();
            let node = app.world().get::<Node>(child)?;
            let at = match (axis, node.left, node.top) {
                (CanvasAxis::Vertical, Val::Px(left), _) => left,
                (CanvasAxis::Horizontal, _, Val::Px(top)) => top,
                _ => return None,
            };
            Some((text, at))
        })
        .collect();
    labels.sort_by(|(_, a), (_, b)| a.total_cmp(b));
    labels
}

/// The guides drawn over one panel: each line's axis and where its hit
/// slab was placed, in the order the scene lists them.
fn guide_lines_of(app: &mut App, panel: Entity) -> Vec<(CanvasAxis, Val)> {
    let mut query = app.world_mut().query::<(&GuideLine, &Node)>();
    let mut lines: Vec<(CanvasAxis, usize, Val)> = query
        .iter(app.world())
        .filter(|(line, _)| line.host == panel)
        .map(|(line, node)| {
            let at = match line.axis {
                CanvasAxis::Vertical => node.left,
                CanvasAxis::Horizontal => node.top,
            };
            (line.axis, line.index, at)
        })
        .collect();
    lines.sort_by_key(|(axis, index, _)| (*axis == CanvasAxis::Horizontal, *index));
    lines.into_iter().map(|(axis, _, at)| (axis, at)).collect()
}

/// A guide comes off a ruler: the press draws one under the cursor and
/// the release leaves it where the pointer stopped.
#[test]
fn dragging_off_a_ruler_creates_a_guide_where_the_pointer_stops() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    settle(&mut app);
    let ruler = ruler_entity(&mut app, panel, CanvasAxis::Vertical);
    let entries = history_len(&app);

    drag_authored(
        &mut app,
        panel,
        ruler,
        Vec2::new(320.0, -10.0),
        Vec2::new(320.0, 400.0),
    );
    settle(&mut app);

    assert_eq!(
        guide_positions(&app, root, CanvasAxis::Vertical),
        vec![320.0],
        "the guide is left on the authored pixel the pointer stopped over",
    );
    assert_eq!(history_len(&app), entries + 1, "one drag is one entry");

    undo(&mut app);
    settle(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "and one undo takes the guide, and the component, back off",
    );
}

#[test]
fn dragging_a_guide_moves_it_and_dragging_it_back_onto_the_ruler_removes_it() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    app.world_mut().entity_mut(root).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![320.0],
    });
    settle(&mut app);

    let line = guide_line_entity(&mut app, panel, CanvasAxis::Vertical, 0);
    let entries = history_len(&app);
    // Both ends of the drag sit on the panel's own lattice, which is
    // what the magnet lands a guide on.
    drag_authored(
        &mut app,
        panel,
        line,
        Vec2::new(320.0, 300.0),
        Vec2::new(504.0, 300.0),
    );
    settle(&mut app);
    assert_eq!(
        guide_positions(&app, root, CanvasAxis::Vertical),
        vec![504.0],
        "the guide follows the cursor",
    );
    assert_eq!(history_len(&app), entries + 1, "and is one entry");

    let line = guide_line_entity(&mut app, panel, CanvasAxis::Vertical, 0);
    drag_authored(
        &mut app,
        panel,
        line,
        Vec2::new(504.0, 300.0),
        Vec2::new(504.0, -10.0),
    );
    settle(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "released over its own ruler, the guide goes back where it came from",
    );
    assert_eq!(history_len(&app), entries + 2, "which is one more entry");
}

#[test]
fn escape_abandons_a_guide_drag() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    settle(&mut app);
    let ruler = ruler_entity(&mut app, panel, CanvasAxis::Vertical);
    let entries = history_len(&app);

    let start = begin_drag(&mut app, panel, ruler, Vec2::new(320.0, -10.0));
    let distance = screen_position_of(&mut app, panel, Vec2::new(320.0, 400.0)) - start;
    continue_drag(&mut app, ruler, start, distance);
    settle(&mut app);
    assert_eq!(
        guide_positions(&app, root, CanvasAxis::Vertical),
        vec![320.0],
        "the guide follows the cursor while the drag runs",
    );

    press_escape(&mut app);
    settle(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "Escape puts the guides back the way the drag found them",
    );

    end_drag(&mut app, ruler, start, distance);
    settle(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "and the release that follows the cancel draws nothing",
    );
    assert_eq!(history_len(&app), entries, "an abandoned drag is no entry");
}

#[test]
fn a_guide_cannot_be_dragged_in_interact_mode() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    settle(&mut app);
    let ruler = ruler_entity(&mut app, panel, CanvasAxis::Vertical);

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .mode = Viewport2dMode::Interact;
    settle(&mut app);
    let entries = history_len(&app);

    drag_authored(
        &mut app,
        panel,
        ruler,
        Vec2::new(320.0, -10.0),
        Vec2::new(320.0, 400.0),
    );
    settle(&mut app);
    assert!(
        app.world().get::<CanvasGuides>(root).is_none(),
        "a panel running the scene draws no guides off its rulers",
    );
    assert_eq!(history_len(&app), entries, "and records nothing");
}

#[test]
fn a_guide_drag_that_did_not_move_records_nothing() {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let (root, _, _) = authored_scene(&mut app);
    app.world_mut().entity_mut(root).insert(CanvasGuides {
        horizontal: Vec::new(),
        vertical: vec![320.0],
    });
    settle(&mut app);

    let line = guide_line_entity(&mut app, panel, CanvasAxis::Vertical, 0);
    let entries = history_len(&app);

    // Out and back inside one gesture: the guide is written on every
    // event, and the history hears about the gesture, not the events.
    let start = begin_drag(&mut app, panel, line, Vec2::new(320.0, 300.0));
    let away = screen_position_of(&mut app, panel, Vec2::new(500.0, 300.0)) - start;
    continue_drag(&mut app, line, start, away);
    settle(&mut app);
    continue_drag(&mut app, line, start, Vec2::ZERO);
    end_drag(&mut app, line, start, Vec2::ZERO);
    settle(&mut app);

    assert_eq!(
        guide_positions(&app, root, CanvasAxis::Vertical),
        vec![320.0],
        "the guide is back where it started",
    );
    assert_eq!(
        history_len(&app),
        entries,
        "so the drag has nothing to record",
    );
}

/// A guide lands on a figure the inspector can state.
///
/// Whole authored pixels with the magnet off, and the panel's own
/// lattice with it on: a guide is the line other things are aimed at, so
/// it has to sit somewhere they can reach.
#[test]
fn a_dragged_guide_lands_on_a_whole_pixel_or_on_the_grid() {
    let dropped = |magnet_on: bool, to: f32| {
        let mut app = stage_app();
        let panel = panel_entity(&mut app);
        let (root, _, _) = authored_scene(&mut app);
        magnet(&mut app, magnet_on);
        set_grid(&mut app, panel, 8.0);
        settle(&mut app);
        let ruler = ruler_entity(&mut app, panel, CanvasAxis::Vertical);
        drag_authored(
            &mut app,
            panel,
            ruler,
            Vec2::new(to, -10.0),
            Vec2::new(to, 400.0),
        );
        settle(&mut app);
        guide_positions(&app, root, CanvasAxis::Vertical)
    };

    assert_eq!(
        dropped(false, 320.37),
        vec![320.0],
        "a guide dropped off the ruler lands on the authored pixel under it",
    );
    assert_eq!(
        dropped(true, 324.37),
        vec![328.0],
        "and on the canvas's own lattice while the magnet is on",
    );
}

/// One of a panel's rulers.
fn ruler_entity(app: &mut App, panel: Entity, axis: CanvasAxis) -> Entity {
    rulers_of(app, panel)
        .into_iter()
        .find(|(_, ruler_axis)| *ruler_axis == axis)
        .map(|(entity, _)| entity)
        .expect("the panel has a ruler on each axis")
}

/// One guide line drawn over a panel.
fn guide_line_entity(app: &mut App, panel: Entity, axis: CanvasAxis, index: usize) -> Entity {
    let mut query = app.world_mut().query::<(Entity, &GuideLine)>();
    query
        .iter(app.world())
        .find(|(_, line)| line.host == panel && line.axis == axis && line.index == index)
        .map(|(entity, _)| entity)
        .expect("the panel draws the scene's guides")
}

/// The scene's guides on one axis, exactly as the scene holds them.
fn guide_positions(app: &App, root: Entity, axis: CanvasAxis) -> Vec<f32> {
    let Some(guides) = app.world().get::<CanvasGuides>(root) else {
        return Vec::new();
    };
    let lines = match axis {
        CanvasAxis::Vertical => &guides.vertical,
        CanvasAxis::Horizontal => &guides.horizontal,
    };
    lines.clone()
}

/// A root filling the canvas with one child authored as `node`. Returns
/// the panel and the child.
fn anchored_app(node: Node) -> (App, Entity, Entity) {
    let mut app = stage_app();
    let panel = panel_entity(&mut app);
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let child = app.world_mut().spawn((node, ChildOf(root))).id();
    settle(&mut app);
    (app, panel, child)
}

/// Select `entity` and hand back the outline the sync spawned over it.
fn outline_over(app: &mut App, entity: Entity) -> Entity {
    select(app, entity);
    settle(app);
    let (overlay, _) = overlay_node(app);
    overlay
}

fn stage_app() -> App {
    util::editor_test_app()
}

/// A panel with a 1200x600 stage area, framed at half zoom so the whole
/// 2400x1200 canvas fits it: two authored pixels per stage pixel.
fn panel_entity(app: &mut App) -> Entity {
    framed_panel(app, 0.5)
}

fn framed_panel(app: &mut App, zoom: f32) -> Entity {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                // The header and the ruler gutter come off the panel
                // before the area is measured, so the panel is grown by
                // both to leave the 1200x600 area every position below
                // is stated against.
                width: px(1200.0 + RULER_SIZE),
                height: px(600.0 + RULER_SIZE + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    host.view.zoom = zoom;
    // An explicit framing: every position below is stated in authored
    // pixels at this zoom, so the fit a new panel starts with has to
    // stand down the way a restored framing does.
    host.fit_pending = false;
    parent
}

/// A root filling the canvas with two overlapping absolutely placed
/// children; `front` is the later sibling.
fn authored_scene(app: &mut App) -> (Entity, Entity, Entity) {
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let back = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(200),
                top: px(100),
                width: px(400),
                height: px(200),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let front = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(400),
                top: px(200),
                width: px(400),
                height: px(200),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    (root, back, front)
}

/// A ten-pixel-bordered container at authored (200, 100), holding a flex
/// child with no offset of its own and an absolutely placed sibling 400
/// pixels into the container's padding box.
///
/// Returns the flex child. Its laid-out corner is the padding box's
/// origin, so any border term leaking into the offset space shows up as
/// a jump of exactly the border width.
fn bordered_scene(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    let container = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(200),
                top: px(100),
                width: px(800),
                height: px(400),
                border: UiRect::all(px(10)),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    let flexed = app
        .world_mut()
        .spawn((
            Node {
                width: px(100),
                height: px(50),
                ..default()
            },
            ChildOf(container),
        ))
        .id();
    app.world_mut().spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(400),
            top: px(0),
            width: px(120),
            height: px(50),
            ..default()
        },
        ChildOf(container),
    ));
    flexed
}

/// Put the canvas's master magnet where the Snap menu's first row puts
/// it. On is the default, so this is only ever called to turn it off.
fn magnet(app: &mut App, on: bool) {
    with_kinds(app, |kinds| kinds.enabled = on);
}

/// Put the panel's canvas grid on a one-pixel lattice, which is no
/// lattice at all once the gesture rounds to whole pixels, so that the
/// edge snap is the only thing that can move the drag.
fn without_the_pixel_grid(app: &mut App, panel: Entity) {
    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .grid = 1.0;
}

fn select(app: &mut App, entity: Entity) {
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn stage_entity(app: &mut App, panel: Entity) -> Entity {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .stage
}

/// Press the primary button over the point on screen showing `authored`
/// (authored pixels, origin at the canvas's top-left corner), targeting
/// the stage node.
fn click_authored(app: &mut App, panel: Entity, authored: Vec2) {
    let stage = stage_entity(app, panel);
    press_at(app, panel, authored, stage);
}

/// Where on screen the panel is currently showing authored point
/// `authored`, in window pixels.
///
/// Derived from the *area* and the view rather than from the stage node
/// the production path measures. The two agree only if `place_stage` put
/// the canvas where the view says it is, so every click test doubles as
/// a check on the placement: drop the zoom from the placement and these
/// positions stop landing on the nodes they name.
fn screen_position_of(app: &mut App, panel: Entity, authored: Vec2) -> Vec2 {
    let (area, view, target_size) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.area, host.view, host.target_size))
        .expect("host on panel parent");
    let computed = *app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out");
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(area)
        .expect("the stage area is laid out")
        .translation;

    // The authored point the view has parked at the centre of the area,
    // in authored pixels from the canvas's top-left corner.
    let focus = target_size.as_vec2() / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    let area_centre_logical = centre * computed.inverse_scale_factor();
    let logical = area_centre_logical + (authored - focus) * view.zoom;
    logical * app.world().resource::<UiScale>().0
}

/// Press the primary button at the screen position showing `authored`,
/// delivering the event to `target`.
fn press_at(app: &mut App, panel: Entity, authored: Vec2, target: Entity) {
    let position = screen_position_of(app, panel, authored);
    let camera = panel_camera(app, panel);
    pointer_at(
        app,
        target,
        position,
        Press {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            count: 1,
        },
    );
}

fn panel_camera(app: &mut App, panel: Entity) -> Entity {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera
}

/// Deliver one pointer event to `target` at a window position.
fn pointer_at<E: std::fmt::Debug + Clone + Reflect>(
    app: &mut App,
    target: Entity,
    position: Vec2,
    event: E,
) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let render_target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: render_target,
            position,
        },
        event,
        target,
    ));
}

fn overlay_node(app: &mut App) -> (Entity, Node) {
    let overlays: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<UiSelectionOverlay>>()
        .iter(app.world())
        .collect();
    assert_eq!(overlays.len(), 1, "exactly one overlay per selection");
    let overlay = overlays[0];
    let node = app
        .world()
        .get::<Node>(overlay)
        .expect("the overlay is a node")
        .clone();
    (overlay, node)
}

fn node_of(app: &App, entity: Entity) -> Node {
    app.world()
        .get::<Node>(entity)
        .expect("the authored entity is a node")
        .clone()
}

fn history_len(app: &App) -> usize {
    app.world()
        .resource::<jackdaw::commands::CommandHistory>()
        .undo_stack
        .len()
}

fn undo(app: &mut App) {
    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw::commands::CommandHistory>| {
            history.undo(world);
        },
    );
}

/// The whole gesture on `target`: press at the screen position showing
/// `from`, drag to the one showing `to`, release.
///
/// The pointer distance comes from `screen_position_of`, so what the
/// gesture believes the cursor moved is derived from the area and the
/// view rather than from the numbers the production path uses. A drag
/// test is therefore also a placement test.
fn drag_authored(app: &mut App, panel: Entity, target: Entity, from: Vec2, to: Vec2) {
    let start = begin_drag(app, panel, target, from);
    let distance = screen_position_of(app, panel, to) - start;
    continue_drag(app, target, start, distance);
    end_drag(app, target, start, distance);
}

fn begin_drag(app: &mut App, panel: Entity, target: Entity, from: Vec2) -> Vec2 {
    let start = screen_position_of(app, panel, from);
    let camera = panel_camera(app, panel);
    pointer_at(
        app,
        target,
        start,
        DragStart {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
        },
    );
    start
}

fn continue_drag(app: &mut App, target: Entity, start: Vec2, distance: Vec2) {
    pointer_at(
        app,
        target,
        start + distance,
        Drag {
            button: PointerButton::Primary,
            distance,
            delta: distance,
        },
    );
}

fn end_drag(app: &mut App, target: Entity, start: Vec2, distance: Vec2) {
    pointer_at(
        app,
        target,
        start + distance,
        DragEnd {
            button: PointerButton::Primary,
            distance,
        },
    );
}

/// Escape as the editor sees it: a keyboard message the input pass turns
/// into `just_pressed` on the frame it is read.
fn press_escape(app: &mut App) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    app.world_mut().write_message(KeyboardInput {
        key_code: KeyCode::Escape,
        logical_key: Key::Escape,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
    app.update();
}

fn handle_entity(app: &mut App, overlay: Entity, want: (i8, i8)) -> Entity {
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(overlay)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .find(|child| {
            app.world()
                .get::<UiResizeHandle>(*child)
                .is_some_and(|handle| (handle.x, handle.y) == want)
        })
        .unwrap_or_else(|| panic!("no {want:?} handle on the overlay"))
}

fn handle_layout(app: &mut App, overlay: Entity) -> Vec<(i8, i8)> {
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(overlay)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    children
        .into_iter()
        .filter_map(|child| {
            app.world()
                .get::<UiResizeHandle>(child)
                .map(|handle| (handle.x, handle.y))
        })
        .collect()
}
