use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;

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
    app.register_extension::<SampleExtension>();
    app.finish();
    // first update sets the extension up
    // todo: maybe do plugin setup in `Startup` so that jackdaw is actually ready in the first frame?
    app.update();
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
    app.register_extension::<SampleExtension>();
    app.finish();
    app.update();

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
    app.register_extension::<SampleExtension>();
    app.finish();
    app.update();
    app.world_mut()
        .operator(SampleExtension::CHECK_PARAMS)
        .param("foo", "bar")
        .param("baz", 42)
        .call()
        .unwrap()
        .assert_finished();
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
