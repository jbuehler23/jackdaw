use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::util::OperatorResultExt as _;
mod util;

#[test]
fn smoke_test_headless_update() {
    let mut app = util::headless_app();
    app.finish();

    for _ in 0..10 {
        app.update();
    }
}

#[test]
fn can_run_extension() {
    let mut app = util::headless_app();
    util::register_and_enable_extension::<SampleExtension>(&mut app);
    for _ in 0..10 {
        app.world_mut()
            .operator(SampleExtension::SPAWN)
            .call()
            .unwrap()
            .assert_finished();
        app.update();
    }
}

#[test]
fn can_call_operator() {
    let mut app = util::headless_app();
    util::register_and_enable_extension::<SampleExtension>(&mut app);

    let amount_of_panels = app
        .world_mut()
        .query_filtered::<(), With<Panel>>()
        .iter(app.world())
        .count();
    // TODO: why is this panel not spawned? What do I need to do in order to make it spawn?
    assert_eq!(amount_of_panels, 0);
    assert!(!app.world_mut().contains_resource::<Marker>());

    app.world_mut()
        .operator(SampleExtension::SPAWN)
        .call()
        .unwrap()
        .assert_finished();

    assert!(app.world_mut().contains_resource::<Marker>());
}

#[test]
fn can_pass_params_to_operator() {
    let mut app = util::headless_app();
    util::register_and_enable_extension::<SampleExtension>(&mut app);
    app.world_mut()
        .operator(SampleExtension::CHECK_PARAMS)
        .param("foo", "bar")
        .param("baz", 42)
        .call()
        .unwrap()
        .assert_finished();
}

// ─────────────────────── brush.* precondition tests ───────────────────────
//
// These pin down the global-state preconditions each brush operator
// documents in its `description`. Contributors copy-pasting these as
// templates for future brush ops should keep the same no-op-on-invalid
// contract.

fn spawn_cuboid_brush(app: &mut App, offset: Vec3) -> Entity {
    use jackdaw_jsn::Brush;
    app.world_mut()
        .spawn((
            Name::new("TestBrush"),
            Brush::cuboid(0.5, 0.5, 0.5),
            Transform::from_translation(offset),
            Visibility::default(),
        ))
        .id()
}

fn with_headless_brush_env<F: FnOnce(&mut App)>(f: F) {
    use bevy::input_focus::InputFocus;
    let mut app = util::headless_app();
    app.finish();
    app.update();
    // The headless app starts with `InputFocus = Some(placeholder)`;
    // the brush ops' availability checks treat that as "a text field
    // owns the keyboard" and refuse to run. Clear it so tests see the
    // same state as an editor with the viewport focused.
    app.world_mut().resource_mut::<InputFocus>().0 = None;
    f(&mut app);
}

#[test]
fn brush_join_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        // Empty selection → not available.
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

        // One selected brush → still not available.
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );

        // Two selected brushes → available.
        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.join")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_csg_subtract_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.csg_subtract")
                .is_available()
                .unwrap()
        );

        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.csg_subtract")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_csg_intersect_unavailable_without_two_brushes() {
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1];
        app.update();
        assert!(
            !app.world_mut()
                .operator("brush.csg_intersect")
                .is_available()
                .unwrap()
        );

        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(
            app.world_mut()
                .operator("brush.csg_intersect")
                .is_available()
                .unwrap()
        );
    });
}

#[test]
fn brush_extend_face_unavailable_without_resolvable_face() {
    use jackdaw::brush::{BrushEditMode, BrushSelection, EditMode};
    use jackdaw::selection::Selection;

    with_headless_brush_env(|app| {
        let op = "brush.extend_face_to_brush";

        // Object mode + empty selection: no primary, no face.
        assert!(!app.world_mut().operator(op).is_available().unwrap());

        // Object mode + 2 brushes but no remembered face: still not
        // available (the op needs either a face-mode pick or a
        // remembered face on the primary).
        let b1 = spawn_cuboid_brush(app, Vec3::ZERO);
        let b2 = spawn_cuboid_brush(app, Vec3::X);
        app.world_mut().resource_mut::<Selection>().entities = vec![b1, b2];
        app.update();
        assert!(!app.world_mut().operator(op).is_available().unwrap());

        // Object mode + 2 brushes + a remembered face on the primary:
        // now available.
        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.last_face_entity = Some(b1);
            brush_selection.last_face_index = Some(0);
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());

        // Face mode with a face picked on the primary and ≥ 1 other
        // brush selected: also available.
        *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
        {
            let mut brush_selection = app.world_mut().resource_mut::<BrushSelection>();
            brush_selection.entity = Some(b1);
            brush_selection.faces = vec![0];
        }
        app.update();
        assert!(app.world_mut().operator(op).is_available().unwrap());
    });
}

/// Verifies that the snapshot mechanism notices changes to editor-state
/// resources (`EditMode`, `GizmoMode`, `ViewModeSettings`, ...). Two
/// snapshots taken either side of a resource mutation must compare
/// unequal — if they compared equal, the operator dispatcher would
/// silently drop the undo entry and Ctrl+Z wouldn't restore the old
/// state. The restore-via-`apply` half of the contract goes through
/// `apply_ast_to_world`, which drives editor UI systems that can't run
/// headless; that half is covered by manual smoke testing in the
/// editor.
#[test]
fn snapshot_notices_editor_state_changes() {
    use jackdaw::brush::{BrushEditMode, EditMode};
    use jackdaw::gizmos::{GizmoMode, GizmoSpace};
    use jackdaw::snapping::SnapSettings;
    use jackdaw::view_modes::ViewModeSettings;
    use jackdaw::viewport_overlays::OverlaySettings;
    use jackdaw_api_internal::snapshot::ActiveSnapshotter;
    use jackdaw_avian_integration::PhysicsOverlayConfig;

    let mut app = util::headless_app();
    app.finish();
    app.update();

    let world = app.world_mut();
    let before = world
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));

    // Flip each editor-state resource the snapshot should cover.
    *world.resource_mut::<ViewModeSettings>() = ViewModeSettings { wireframe: true };
    *world.resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
    *world.resource_mut::<GizmoMode>() = GizmoMode::Rotate;
    *world.resource_mut::<GizmoSpace>() = GizmoSpace::Local;
    world.resource_mut::<SnapSettings>().grid_power = 3;
    world.resource_mut::<OverlaySettings>().show_bounding_boxes = true;
    world.resource_mut::<PhysicsOverlayConfig>().show_colliders = false;

    let after = world
        .resource_scope(|world, snapshotter: Mut<ActiveSnapshotter>| snapshotter.0.capture(world));
    assert!(
        !before.equals(&*after),
        "snapshotter should observe the mutated editor-state resources"
    );
}

#[derive(Default)]
struct SampleExtension;

impl SampleExtension {
    const SPAWN: &'static str = "sample.spawn";
    const CHECK_PARAMS: &'static str = "sample.check_params";
}

impl JackdawExtension for SampleExtension {
    fn id() -> String {
        "sample".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(WindowDescriptor {
            id: Self::SPAWN.into(),
            build: Arc::new(build_panel),
            default_area: Some("left".into()),
            ..default()
        });
        ctx.register_operator::<SpawnMarkerOp>()
            .register_operator::<CheckParamsOp>();
    }
}

fn build_panel(world: &mut World, parent: Entity) {
    world.spawn((ChildOf(parent), Panel, Text::new("Some panel")));
}

#[operator(
    id = SampleExtension::SPAWN,
)]
fn spawn_marker(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.init_resource::<Marker>();
    OperatorResult::Finished
}

#[operator(
    id = SampleExtension::CHECK_PARAMS,
)]
fn check_params(params: In<OperatorParameters>) -> OperatorResult {
    assert_eq!(params["foo"], "bar".into());
    assert_eq!(params["baz"], 42.into());
    OperatorResult::Finished
}

#[derive(Resource, Default)]
struct Marker;

#[derive(Component)]
struct Panel;
