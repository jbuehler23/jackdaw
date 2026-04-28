//! Extends the `minimal_operator` example to add a keybinding for the operator.
//! Keybindings in Jackdaw use `bevy_enhanced_input` (BEI), so before reading this example,
//! you should take a look at its documentation at <https://docs.rs/bevy_enhanced_input/latest/bevy_enhanced_input/>.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<KeybindingExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct KeybindingExampleExtension;

impl JackdawExtension for KeybindingExampleExtension {
    fn id(&self) -> String {
        "keybinding_example".to_string()
    }

    fn label(&self) -> String {
        "Keybinding Example".to_string()
    }

    fn description(&self) -> String {
        "Adds an operator that logs the elapsed seconds since Jackdaw started that can be called by pressing the F10 key.".to_string()
    }

    // Input contexts must be registered only once per application at startup.
    // Since `register` runs at runtime every time the extension is enabled by the user,
    // we use this special `register_input_context` method to register the input context at startup.
    fn register_input_context(&self, app: &mut App) {
        app.add_input_context::<KeybindingExampleInputContext>();
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<ElapsedSecondsOp>().spawn((
            KeybindingExampleInputContext,
            // Each operator is also a BEI `InputAction` that is wired up for you to call the operator.
            actions!(
                KeybindingExampleInputContext[(
                    Action::<ElapsedSecondsOp>::new(),
                    // Usually, operators use `Press` to only be triggered once when the button is pressed down.
                    // Due to a name conflict, you need to import it explicitly with `use bevy_enhanced_input::prelude::{Press, *};`
                    //
                    // This keybinding is used as a default, but a user can override it themselves in the settings.
                    bindings![(KeyCode::F10, Press::default())]
                )]
            ),
        ));
    }
}

/// Every extension should have its own input context so that it can be cleaned up properly when the extension is unregistered.
#[derive(Component)]
struct KeybindingExampleInputContext;

#[operator(
    id = "keybinding_example.elapsed_seconds",
    label = "Log Elapsed Seconds",
    description = "Logs the elapsed seconds since Jackdaw started."
)]
// Press F10 to invoke this operator and see a log message in the console.
fn elapsed_seconds(_: In<OperatorParameters>, time: Res<Time>) -> OperatorResult {
    info!("Elapsed seconds: {}", time.elapsed_secs());
    OperatorResult::Finished
}
