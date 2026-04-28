//! Shows how to call a builtin operator from a custom operator.

use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<CallOperatorExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct CallOperatorExampleExtension;

impl JackdawExtension for CallOperatorExampleExtension {
    fn id(&self) -> String {
        "call_operator_example".to_string()
    }

    fn label(&self) -> String {
        "Call Operator Example".to_string()
    }

    fn description(&self) -> String {
        "Adds a cube by calling a builtin operator for it.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        // This operator is registered without any UI, so use F3 and search for "Spawn a cube" to find it.
        ctx.register_operator::<SpawnCubeOp>();
    }
}

#[operator(
    id = "call_operator_example.spawn_cube",
    label = "Spawn a cube",
    description = "Spawn a cube at the origin of the scene by using the builtin cube spawning operator."
)]
fn spawn_cube(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    // Call the builtin cube spawning operator by its ID.
    // See <https://github.com/jbuehler23/jackdaw/tree/main/docs/operators.md>
    // for a list of builtin operators and their IDs.
    commands.operator("entity.add.cube").call();

    OperatorResult::Finished
}
