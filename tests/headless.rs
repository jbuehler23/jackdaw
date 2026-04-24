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
