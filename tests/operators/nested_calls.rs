//! An operator's system can call other operators, through the world or
//! through `Commands`, and each nested call runs.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::util;
use crate::util::OperatorResultExt as _;

#[derive(Resource, Default)]
struct InnerRuns(usize);

#[derive(Default)]
struct NestingExtension;

impl NestingExtension {
    const INNER: &'static str = "nesting.inner";
    const THROUGH_WORLD: &'static str = "nesting.through_world";
    const THROUGH_COMMANDS: &'static str = "nesting.through_commands";
}

impl JackdawExtension for NestingExtension {
    fn id(&self) -> String {
        "nesting".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<InnerOp>()
            .register_operator::<ThroughWorldOp>()
            .register_operator::<ThroughCommandsOp>();
    }
}

#[operator(id = NestingExtension::INNER, allows_undo = false)]
fn inner(_: In<OperatorParameters>, mut runs: ResMut<InnerRuns>) -> OperatorResult {
    runs.0 += 1;
    OperatorResult::Finished
}

#[operator(id = NestingExtension::THROUGH_WORLD, allows_undo = false)]
fn through_world(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match world.operator(NestingExtension::INNER).call() {
        Ok(OperatorResult::Finished) => OperatorResult::Finished,
        _ => OperatorResult::Cancelled,
    }
}

#[operator(id = NestingExtension::THROUGH_COMMANDS, allows_undo = false)]
fn through_commands(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.operator(NestingExtension::INNER).call();
    OperatorResult::Finished
}

#[test]
fn an_operator_calls_another_through_the_world_or_through_commands() {
    let mut app = util::headless_app();
    app.init_resource::<InnerRuns>();
    util::register_and_enable_extension::<NestingExtension>(&mut app);

    app.world_mut()
        .operator(NestingExtension::THROUGH_WORLD)
        .call()
        .expect("the outer call dispatches")
        .assert_finished();
    assert_eq!(
        app.world().resource::<InnerRuns>().0,
        1,
        "the nested world call ran"
    );

    app.world_mut()
        .operator(NestingExtension::THROUGH_COMMANDS)
        .call()
        .expect("the outer call dispatches")
        .assert_finished();
    app.update();
    assert_eq!(
        app.world().resource::<InnerRuns>().0,
        2,
        "the queued call ran"
    );
}
