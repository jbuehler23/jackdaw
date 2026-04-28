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
    fn id(&self) -> String {
        "sample".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(Self::SPAWN)
                .with_default_area(DefaultArea::Left)
                .with_build(|window| {
                    window.spawn((Panel, Text::new("Some panel")));
                }),
        );
        ctx.register_operator::<SpawnMarkerOp>()
            .register_operator::<CheckParamsOp>();
    }
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
