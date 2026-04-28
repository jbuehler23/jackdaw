//! Shows how to register a minimal operator in a Jackdaw extension.

use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<OperatorExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct OperatorExampleExtension;

impl JackdawExtension for OperatorExampleExtension {
    fn id(&self) -> String {
        "operator_example".to_string()
    }

    fn label(&self) -> String {
        "Minimal Operator Example".to_string()
    }

    fn description(&self) -> String {
        "Adds a simple operator that logs the elapsed seconds since Jackdaw started.".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        // Operators are bevy systems created with `#[operator]`, which creates a struct named like the system with `Op` appended.
        // You can call this operator from the editor by pressing F3 to open the operator search.
        // Enter "Log Elapsed Seconds" to find this operator and click on it to run it.
        ctx.register_operator::<ElapsedSecondsOp>();
    }
}

#[operator(
    id = "operator_example.elapsed_seconds",
    label = "Log Elapsed Seconds",
    description = "Logs the elapsed seconds since Jackdaw started."
)]
// This is a regular Bevy system that gets invoked by jackdaw when the operator is called.
// You can add whatever queries, resources, etc. you like.
// The only rules are
// - the first parameter must be `In<OperatorParameters>`, which contains a key-value map of parameters your operator may support
// - the return type must be `OperatorResult`. The most important variants are
//   - `OperatorResult::Cancelled` for when something went wrong and the operator should be cancelled
//   - `OperatorResult::Finished` for when the operator has completed successfully
// - The ID used for the operator must be unique. Usually this is done by naming it with the schema `{extension_id}.{system_name}`
fn elapsed_seconds(_: In<OperatorParameters>, time: Res<Time>) -> OperatorResult {
    info!("Elapsed seconds: {}", time.elapsed_secs());
    OperatorResult::Finished
}
